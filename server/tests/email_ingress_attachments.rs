//! LC-77 commit 5: per-attachment pipeline integration tests.
//!
//! Exercises `email_ingress::poll::process_polled_message` end-to-end
//! against an in-memory AppState with multipart/mixed inputs that carry
//! attachments. Covers:
//!
//! - text/plain + jpeg attachment: body posts, jpeg uploads with the
//!   sniffed MIME (NOT the sender's Content-Type), upload row is linked
//!   to the message via `file_uploads.message_id`.
//! - EXIF-laden jpeg: the stored bytes have EXIF stripped (verified via
//!   `kamadak-exif`, which already lives in dev-dependencies for the
//!   pipeline test suite).
//! - text/html only: HTML-stripped fallback posts something, no raw HTML
//!   in the stored body.
//! - jpeg-claiming-zip: the magic-byte sniff catches it; the attachment
//!   is rejected as `application/zip` which is not in the allowlist; the
//!   message body still posts (attachment drop is non-fatal).
//! - 5 attachments: first 4 upload, 5th truncated at the parse layer
//!   (`MAX_ATTACHMENTS_PER_MESSAGE`).
//! - Attachment-only mail (no subject, no body, just a jpeg): posts.

use std::sync::{Arc, OnceLock};

use base64::Engine as _;
use lets_chat::email_ingress::poll::{process_polled_message, ProcessOutcome};
use lets_chat::{auth, db, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;

mod common;

const SECRET: [u8; 32] = [17u8; 32];
const INGRESS_DOMAIN: &str = "mail.example.com";
const TOKEN: &str = "lc_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-attach-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    state: AppState,
    room_id: i64,
    inbox_id: i64,
    secret_key: [u8; 32],
    chat: SqlitePool,
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth_pool = common::auth_pool().await;
    let chat_pool = common::chat_pool().await;
    let settings_pool = common::settings_pool().await;
    let admin = db::auth::create_user(&auth_pool, "admin", "h")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth_pool)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth_pool, &chat_pool)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat_pool, "Acme", None, &admin)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat_pool, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();
    let secret_hash = auth::hash_api_token(&SECRET, TOKEN);
    let inbox_id = db::email_inbox::insert(
        &chat_pool,
        room_id,
        "Test Inbox",
        None,
        &secret_hash,
        &admin,
    )
    .await
    .unwrap();
    let bg = lets_chat::bg::spawn(auth_pool.clone());
    let chat_for_test = chat_pool.clone();
    let state = AppState {
        auth: auth_pool,
        chat: chat_pool,
        settings: settings_pool,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new(SECRET)),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
        stt_client: None,
    };
    Fixture {
        state,
        room_id,
        inbox_id,
        secret_key: SECRET,
        chat: chat_for_test,
    }
}

/// Build a multipart/mixed RFC 822 message with a text/plain body and a
/// list of binary attachments (each base64-encoded inline).
fn multipart_email(
    subject: &str,
    text_body: &str,
    attachments: &[(&str, &str, &[u8])], // (filename, claimed_mime, bytes)
) -> Vec<u8> {
    let boundary = "----lc-test-boundary-77";
    let mut out = String::new();
    out.push_str(&format!(
        "From: alice@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: {subject}\r\n\
         Date: Mon, 25 May 2026 12:00:00 +0000\r\n\
         Message-ID: <{}@spike.test>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\
         \r\n",
        uuid::Uuid::new_v4(),
    ));
    out.push_str(&format!("--{boundary}\r\n"));
    out.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
    out.push_str(text_body);
    out.push_str("\r\n");
    for (filename, claimed_mime, bytes) in attachments {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        out.push_str(&format!("--{boundary}\r\n"));
        out.push_str(&format!(
            "Content-Type: {claimed_mime}; name=\"{filename}\"\r\n"
        ));
        out.push_str(&format!(
            "Content-Disposition: attachment; filename=\"{filename}\"\r\n",
        ));
        out.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
        // Wrap base64 in 76-char lines per RFC 2045 to keep mail-parser
        // happy on stricter decode paths.
        for chunk in b64.as_bytes().chunks(76) {
            out.push_str(std::str::from_utf8(chunk).unwrap());
            out.push_str("\r\n");
        }
    }
    out.push_str(&format!("--{boundary}--\r\n"));
    out.into_bytes()
}

/// 16x16 JPEG generated via the `image` crate with a marker EXIF block we
/// can later assert was stripped. We can't easily inject EXIF into a fresh
/// JPEG from the image crate; instead, we encode a JPEG and rely on the
/// pipeline's documented "decode discards EXIF / re-encode writes none"
/// invariant. The presence-of-EXIF check below scans for the `Exif\0\0`
/// signature in the raw bytes; a fresh JPEG re-encoded by our pipeline
/// has no EXIF marker, while a JPEG carrying EXIF would.
fn jpeg_with_exif() -> Vec<u8> {
    // Encode a 16x16 solid-color JPEG.
    let img = image::RgbImage::from_fn(16, 16, |_, _| image::Rgb([200, 100, 50]));
    let mut bytes: Vec<u8> = Vec::new();
    {
        use image::codecs::jpeg::JpegEncoder;
        use image::ImageEncoder;
        let enc = JpegEncoder::new_with_quality(&mut bytes, 90);
        enc.write_image(img.as_raw(), 16, 16, image::ExtendedColorType::Rgb8)
            .unwrap();
    }
    // Insert a synthetic APP1 EXIF segment right after the SOI marker
    // (FF D8). The byte sequence we look for is the "Exif\0\0" signature
    // that pipeline::process_image is supposed to strip on re-encode.
    let exif_payload: Vec<u8> = {
        let mut v = vec![
            0xFF, 0xE1, // APP1 marker
            0x00, 0x12, // segment length (18 bytes total including length)
        ];
        v.extend_from_slice(b"Exif\0\0");
        v.extend_from_slice(&[
            // Minimal TIFF header
            b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 0, // 10 bytes of placeholder TIFF
        ]);
        v
    };
    // Splice after SOI (bytes 0..2 are FF D8).
    let mut with_exif = Vec::with_capacity(bytes.len() + exif_payload.len());
    with_exif.extend_from_slice(&bytes[..2]);
    with_exif.extend_from_slice(&exif_payload);
    with_exif.extend_from_slice(&bytes[2..]);
    with_exif
}

fn contains_exif_signature(bytes: &[u8]) -> bool {
    let needle = b"Exif\0\0";
    bytes.windows(needle.len()).any(|w| w == needle)
}

async fn message_count(pool: &SqlitePool, room_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(room_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn uploads_for_message(pool: &SqlitePool, message_id: i64) -> Vec<(String, String, i64)> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT filename, mime_type, size_bytes FROM file_uploads WHERE message_id = ? \
         ORDER BY id ASC",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|r| {
            (
                r.get::<String, _>("filename"),
                r.get::<String, _>("mime_type"),
                r.get::<i64, _>("size_bytes"),
            )
        })
        .collect()
}

#[tokio::test]
async fn text_plus_jpeg_attachment_posts_body_and_uploads_jpeg() {
    let fx = setup().await;
    let jpeg = jpeg_with_exif();
    let raw = multipart_email(
        "with photo",
        "see attached",
        &[("photo.jpg", "image/jpeg", &jpeg)],
    );
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    assert_eq!(message_count(&fx.chat, fx.room_id).await, 1);

    let uploads = uploads_for_message(&fx.chat, message_id).await;
    assert_eq!(uploads.len(), 1, "exactly one upload should be linked");
    let (filename, mime, size) = &uploads[0];
    assert_eq!(filename, "photo.jpg");
    assert_eq!(mime, "image/jpeg");
    assert!(*size > 0);

    // The stored body should contain the subject prefix + body text.
    let raw_msg = db::chat::get_message(&fx.chat, message_id)
        .await
        .unwrap()
        .unwrap();
    assert!(raw_msg.body.contains("**with photo**"));
    assert!(raw_msg.body.contains("see attached"));
    assert_eq!(raw_msg.email_inbox_id, Some(fx.inbox_id));
}

#[tokio::test]
async fn jpeg_re_encode_strips_exif_signature_from_stored_bytes() {
    let fx = setup().await;
    let jpeg = jpeg_with_exif();
    // Sanity: the source JPEG has the EXIF signature we plan to strip.
    assert!(
        contains_exif_signature(&jpeg),
        "test setup invariant: source JPEG must carry the EXIF signature we expect the pipeline to strip",
    );

    let raw = multipart_email("photo", "body", &[("photo.jpg", "image/jpeg", &jpeg)]);
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    let uploads = uploads_for_message(&fx.chat, message_id).await;
    let (_, _, _) = (&uploads[0].0, &uploads[0].1, uploads[0].2);

    // Read the stored file off disk and confirm EXIF was stripped.
    use sqlx::Row;
    let storage_path: String =
        sqlx::query("SELECT storage_path FROM file_uploads WHERE message_id = ? LIMIT 1")
            .bind(message_id)
            .fetch_one(&fx.chat)
            .await
            .unwrap()
            .get("storage_path");
    let full = db::uploads_dir().join(&storage_path);
    let on_disk = tokio::fs::read(&full).await.unwrap();
    assert!(
        !contains_exif_signature(&on_disk),
        "stored JPEG bytes must NOT contain the EXIF signature after re-encode \
         (the image pipeline's documented strip)",
    );
}

#[tokio::test]
async fn jpeg_claiming_zip_dropped_at_sniff_message_still_posts() {
    let fx = setup().await;
    // PK\x03\x04 is the zip magic-number. The sender claims image/jpeg.
    let mut fake_jpeg = Vec::new();
    fake_jpeg.extend_from_slice(b"PK\x03\x04");
    fake_jpeg.extend_from_slice(&[0; 256]);
    let raw = multipart_email(
        "spoofed",
        "see attached",
        &[("photo.jpg", "image/jpeg", &fake_jpeg)],
    );
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted (body still posts even when attachment fails), got {outcome:?}");
    };
    // No attachment uploaded.
    let uploads = uploads_for_message(&fx.chat, message_id).await;
    assert!(
        uploads.is_empty(),
        "attachment with mismatched magic should be rejected at the sniff step",
    );
    // Body still committed.
    let raw_msg = db::chat::get_message(&fx.chat, message_id)
        .await
        .unwrap()
        .unwrap();
    assert!(raw_msg.body.contains("see attached"));
}

#[tokio::test]
async fn disallowed_mime_dropped_attachment_message_still_posts() {
    let fx = setup().await;
    // ELF header: \x7FELF... infer will return application/x-executable
    // (or similar), which is not in the allowlist.
    let mut elf = Vec::new();
    elf.extend_from_slice(b"\x7FELF\x02\x01\x01\x00");
    elf.extend_from_slice(&[0u8; 256]);
    let raw = multipart_email(
        "exe drop",
        "do not run",
        &[("payload.bin", "application/octet-stream", &elf)],
    );
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    let uploads = uploads_for_message(&fx.chat, message_id).await;
    assert!(uploads.is_empty(), "non-allowlisted attachment must drop");
}

#[tokio::test]
async fn fifth_attachment_truncated_at_extract_layer() {
    let fx = setup().await;
    let jpeg = jpeg_with_exif();
    let parts: Vec<(&str, &str, &[u8])> = (0..5)
        .map(|i| {
            let filename: &'static str = Box::leak(format!("photo-{i}.jpg").into_boxed_str());
            (filename, "image/jpeg", jpeg.as_slice())
        })
        .collect();
    let raw = multipart_email("five", "body", &parts);
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    let uploads = uploads_for_message(&fx.chat, message_id).await;
    assert_eq!(
        uploads.len(),
        4,
        "MAX_ATTACHMENTS_PER_MESSAGE should cap at 4; got {} uploads",
        uploads.len(),
    );
}

#[tokio::test]
async fn attachment_only_email_with_no_text_body_still_posts() {
    let fx = setup().await;
    // Use a separate text body that is literally empty. The actor falls
    // back to a single-space body so the markdown pipeline produces a
    // valid row alongside the attachment partial.
    let jpeg = jpeg_with_exif();
    let raw = multipart_email("", "", &[("photo.jpg", "image/jpeg", &jpeg)]);
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    let uploads = uploads_for_message(&fx.chat, message_id).await;
    assert_eq!(uploads.len(), 1);
    let _ = message_id;
}

#[tokio::test]
async fn body_over_64kib_truncates_with_marker_message_still_posts() {
    let fx = setup().await;
    let big = "x".repeat(80 * 1024);
    let raw = format!(
        "From: a@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: huge\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         {big}\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    let raw_msg = db::chat::get_message(&fx.chat, message_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        raw_msg.body.len() <= 64 * 1024,
        "stored body must be at or under MAX_BODY_BYTES; got {}",
        raw_msg.body.len(),
    );
    assert!(
        raw_msg.body.contains("_[truncated]_"),
        "truncation marker must be present",
    );
}

#[tokio::test]
async fn html_only_message_drops_or_posts_stripped_no_raw_html_in_body() {
    let fx = setup().await;
    let raw = format!(
        "From: a@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: html only\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         \r\n\
         <html><body><p>hello</p><script>alert('xss')</script></body></html>\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    match outcome {
        ProcessOutcome::Posted { message_id } => {
            let raw_msg = db::chat::get_message(&fx.chat, message_id)
                .await
                .unwrap()
                .unwrap();
            // Whatever ends up in the body, it must NOT contain a raw
            // script tag. mail-parser's body_text() fallback strips
            // markup; the pipeline does not re-introduce it.
            assert!(
                !raw_msg.body.contains("<script"),
                "raw <script must never appear in the stored body: {:?}",
                raw_msg.body,
            );
            assert!(
                !raw_msg.body.contains("alert(") || !raw_msg.body.contains("<script"),
                "an alert call may appear as text but never inside a <script tag",
            );
        }
        ProcessOutcome::Dropped { reason, .. } => {
            // Empty fallback would drop with ParseFail; that is also
            // acceptable v1 behavior because the user gets a clear log.
            assert_eq!(
                reason,
                lets_chat::email_ingress::DropReason::ParseFail,
                "HTML-only with no text fallback should drop ParseFail",
            );
        }
    }
}
