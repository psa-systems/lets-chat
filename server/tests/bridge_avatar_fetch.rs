//! LC-78-AVATAR-PROXY: fetch module round-trip + failure paths.
//!
//! Spawns a local HTTP receiver returning controlled bytes, points
//! `fetch_and_cache_unchecked` (test seam that skips the LC-152 SSRF
//! re-resolve so loopback works) at it, and asserts the cache row +
//! on-disk file land as expected for each scenario.

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Response, StatusCode};
use axum::routing::get;
use axum::Router;
use lets_chat::{bridge_avatar, db};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tokio::net::TcpListener;

mod common;

fn ensure_tempdir() -> String {
    static INIT: OnceLock<String> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-bavfetch-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
        p.to_string_lossy().into_owned()
    })
    .clone()
}

#[derive(Clone)]
struct Fixture {
    status: u16,
    content_type: String,
    body: Arc<Vec<u8>>,
}

async fn spawn_receiver(fixture: Fixture) -> String {
    let state = fixture.clone();
    let app = Router::new()
        .route(
            "/avatar",
            get(|State(s): State<Fixture>| async move {
                Response::builder()
                    .status(StatusCode::from_u16(s.status).unwrap())
                    .header(header::CONTENT_TYPE, s.content_type.clone())
                    .body(Body::from((*s.body).clone()))
                    .unwrap()
            }),
        )
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/avatar")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("lets-chat-bridge-avatar/test")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn tiny_png() -> Vec<u8> {
    use image::ImageEncoder;
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(&img, 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    buf
}

async fn stage_pending(chat: &SqlitePool, hash: &str, url: &str) {
    db::bridge_avatar_proxies::upsert_pending(chat, hash, url)
        .await
        .unwrap();
}

#[tokio::test]
async fn fetch_round_trip_writes_bytes_and_marks_ok() {
    ensure_tempdir();
    let chat = common::pool("chat").await;
    let png = tiny_png();
    let url = spawn_receiver(Fixture {
        status: 200,
        content_type: "image/png".into(),
        body: Arc::new(png.clone()),
    })
    .await;
    let hash = "aa".repeat(32); // 64-char synthetic key
    stage_pending(&chat, &hash, &url).await;
    bridge_avatar::fetch_and_cache_unchecked(&chat, &client(), &hash, &url).await;
    let row = db::bridge_avatar_proxies::find_by_hash(&chat, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.fetch_status, "ok",
        "failure_reason: {:?}",
        row.failure_reason
    );
    assert_eq!(row.content_type.as_deref(), Some("image/png"));
    assert!(row.byte_size.unwrap_or(0) > 0);
    let path = db::bridge_avatars_dir().join(&hash);
    let on_disk = tokio::fs::read(&path).await.unwrap();
    // The pipeline re-encodes the original to strip metadata, so the bytes
    // on disk are NOT byte-identical to the input. They should decode as a
    // valid PNG with the same dimensions.
    let decoded = image::ImageReader::new(std::io::Cursor::new(&on_disk))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(decoded.width(), 1);
    assert_eq!(decoded.height(), 1);
}

#[tokio::test]
async fn http_404_marks_failed() {
    ensure_tempdir();
    let chat = common::pool("chat").await;
    let url = spawn_receiver(Fixture {
        status: 404,
        content_type: "text/plain".into(),
        body: Arc::new(b"not found".to_vec()),
    })
    .await;
    let hash = "bb".repeat(32);
    stage_pending(&chat, &hash, &url).await;
    bridge_avatar::fetch_and_cache_unchecked(&chat, &client(), &hash, &url).await;
    let row = db::bridge_avatar_proxies::find_by_hash(&chat, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.fetch_status, "failed");
    assert!(row.failure_reason.as_deref().unwrap_or("").contains("404"));
}

#[tokio::test]
async fn non_image_bytes_marks_failed_via_magic_byte_sniff() {
    // Foreign server claims image/png but actually sends a ZIP. Sniff via
    // `infer::get_from_path` ignores the lying Content-Type and rejects.
    ensure_tempdir();
    let chat = common::pool("chat").await;
    let url = spawn_receiver(Fixture {
        status: 200,
        content_type: "image/png".into(),
        body: Arc::new(b"PK\x03\x04\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00".to_vec()),
    })
    .await;
    let hash = "cc".repeat(32);
    stage_pending(&chat, &hash, &url).await;
    bridge_avatar::fetch_and_cache_unchecked(&chat, &client(), &hash, &url).await;
    let row = db::bridge_avatar_proxies::find_by_hash(&chat, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.fetch_status, "failed");
    assert!(
        row.failure_reason
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("mime")
            || row
                .failure_reason
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains("sniff")
    );
}

#[tokio::test]
async fn oversize_payload_marks_failed_at_byte_cap() {
    // 2 MiB payload: well over the 1 MiB cap. The streamed reader rejects
    // mid-stream without fully buffering.
    ensure_tempdir();
    let chat = common::pool("chat").await;
    let big = vec![0xAAu8; 2 * 1024 * 1024];
    let url = spawn_receiver(Fixture {
        status: 200,
        content_type: "image/png".into(),
        body: Arc::new(big),
    })
    .await;
    let hash = "dd".repeat(32);
    stage_pending(&chat, &hash, &url).await;
    bridge_avatar::fetch_and_cache_unchecked(&chat, &client(), &hash, &url).await;
    let row = db::bridge_avatar_proxies::find_by_hash(&chat, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.fetch_status, "failed");
    assert!(row.failure_reason.as_deref().unwrap_or("").contains("1MiB"));
}

#[tokio::test]
async fn canonical_hash_deterministic_for_equivalent_urls() {
    // The cache key dedupe relies on canonicalization producing the same
    // hash for equivalent URLs. Scheme + host should be lowercased; fragment
    // stripped.
    let a = bridge_avatar::canonical_hash("https://EXAMPLE.com/avatar.png").unwrap();
    let b = bridge_avatar::canonical_hash("https://example.com/avatar.png#section").unwrap();
    let c = bridge_avatar::canonical_hash("https://example.com/avatar.png").unwrap();
    assert_eq!(a, b);
    assert_eq!(b, c);
    // Path case IS preserved (Matrix media URLs are case-sensitive in path).
    let d = bridge_avatar::canonical_hash("https://example.com/AVATAR.png").unwrap();
    assert_ne!(d, c, "path case must NOT be normalized");
}
