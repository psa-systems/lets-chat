//! LC-95 integration: admin backup / restore.
//!
//! Covers the round-trip (`build_archive` -> `verify_archive`), the
//! swap-on-startup helper (`apply_pending_restore`), and the two
//! refusal paths (tampered sha256 + mismatched version). The admin
//! HTTP routes are exercised via the live router so the multipart
//! upload + streaming download paths are also covered.

#[cfg(feature = "standalone")]
use axum::body::{to_bytes, Body};
#[cfg(feature = "standalone")]
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{backup, db, routes, state::AppState, ws::hub::Hub};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
#[cfg(feature = "standalone")]
use tower::ServiceExt;

/// File-backed SQLite pool under `dir`. The backup tests use file
/// DBs (not in-memory) because SQLite's `VACUUM INTO` is silently
/// flaky against an `sqlite::memory:` source via sqlx - the
/// statement appears to run (non-zero `rows_affected`) but never
/// produces a destination file. File-backed DBs match production
/// behavior and side-step the quirk.
async fn file_pool(dir: &std::path::Path, domain: &str) -> SqlitePool {
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
    use std::str::FromStr;
    use std::time::Duration;

    let path = dir.join(format!("{domain}.db"));
    let url = format!("sqlite:{}", path.display());
    let opts = SqliteConnectOptions::from_str(&url)
        .unwrap()
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .min_connections(1)
        .connect_with(opts)
        .await
        .expect("pool");
    match domain {
        "auth" => sqlx::migrate!("./migrations/auth")
            .run(&pool)
            .await
            .unwrap(),
        "chat" => sqlx::migrate!("./migrations/chat")
            .run(&pool)
            .await
            .unwrap(),
        "settings" => sqlx::migrate!("./migrations/settings")
            .run(&pool)
            .await
            .unwrap(),
        _ => unreachable!(),
    };
    pool
}

/// Each test gets its own tempdir + sets the global DATA_DIR to it
/// via `db::set_data_dir`. `OnceLock` means the first test to call
/// wins; later tests inherit the same directory but their fixtures
/// land in per-test subdirs so they don't collide.
fn ensure_data_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-backup-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir test root");
        db::set_data_dir(p.to_string_lossy().into_owned());
        p
    })
    .clone()
}

#[allow(dead_code)]
struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    auth: SqlitePool,
    chat: SqlitePool,
    settings: SqlitePool,
}

async fn app() -> TestApp {
    let root = ensure_data_root();
    // Per-test subdir so parallel tests don't collide on the shared
    // global DATA_DIR (the data_dir global is a OnceLock so the
    // first test wins; sub-dir isolation lets others coexist).
    let test_dir = root.join(format!("t-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&test_dir).unwrap();
    let auth = file_pool(&test_dir, "auth").await;
    let chat = file_pool(&test_dir, "chat").await;
    let settings = file_pool(&test_dir, "settings").await;
    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&member_id)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth: auth.clone(),
        chat: chat.clone(),
        settings: settings.clone(),
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
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
        llm_client: None,
        embedding_client: None,
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        auth,
        chat,
        settings,
    }
}

#[cfg(feature = "standalone")]
async fn read_body(res: axum::http::Response<Body>) -> (StatusCode, Vec<u8>) {
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 64 << 20).await.unwrap();
    (status, bytes.to_vec())
}

// ── Module-level helpers ─────────────────────────────────────────────────

#[tokio::test]
async fn build_archive_then_verify_round_trips() {
    let t = app().await;
    // Seed a tiny upload fixture so the walker picks it up.
    let _ = ensure_data_root();
    let uploads_dir = db::uploads_dir();
    std::fs::write(uploads_dir.join("hello.txt"), b"hi").unwrap();

    let out = std::env::temp_dir().join(format!("lc-backup-rt-{}.zip", uuid::Uuid::new_v4()));
    let manifest = backup::build_archive(
        &t.auth,
        &t.chat,
        &t.settings,
        &PathBuf::from(db::data_dir()),
        &out,
    )
    .await
    .unwrap();

    assert!(
        manifest
            .files
            .iter()
            .any(|f| f.path == "auth.db" && f.size > 0),
        "manifest should list auth.db"
    );
    assert!(manifest.files.iter().any(|f| f.path == "chat.db"));
    assert!(manifest.files.iter().any(|f| f.path == "settings.db"));
    assert!(
        manifest
            .files
            .iter()
            .any(|f| f.path == "uploads/hello.txt" && f.size == 2),
        "manifest should list the seeded upload"
    );

    // Re-verify the archive end-to-end.
    let parsed = backup::verify_archive(&out).expect("verify should accept fresh archive");
    assert_eq!(parsed.version, manifest.version);
    let _ = std::fs::remove_file(&out);
}

#[tokio::test]
async fn verify_archive_rejects_tampered_payload() {
    let t = app().await;
    let out = std::env::temp_dir().join(format!("lc-backup-tamper-{}.zip", uuid::Uuid::new_v4()));
    backup::build_archive(
        &t.auth,
        &t.chat,
        &t.settings,
        &PathBuf::from(db::data_dir()),
        &out,
    )
    .await
    .unwrap();

    // Flip a byte deep in the zip. ZIP's own CRC will still reject
    // most catastrophic corruptions; this tests the manifest-sha256
    // gate by patching past the local-file-header CRC field into the
    // payload region.
    let bytes = std::fs::read(&out).unwrap();
    let mut patched = bytes.clone();
    // Pick an offset well past any header; flip one bit.
    let off = patched.len() / 2;
    patched[off] ^= 0xFF;
    std::fs::write(&out, &patched).unwrap();

    let err = backup::verify_archive(&out).expect_err("tampered archive must be rejected");
    let msg = format!("{err:?}").to_lowercase();
    // Could be a CRC / checksum error from the zip reader OR a
    // sha256 mismatch from our manifest gate; either is fine, both
    // are "not the archive the manifest describes".
    assert!(
        msg.contains("sha256")
            || msg.contains("mismatch")
            || msg.contains("crc")
            || msg.contains("checksum"),
        "expected integrity rejection, got: {msg}"
    );
    let _ = std::fs::remove_file(&out);
}

#[tokio::test]
async fn verify_archive_rejects_mismatched_version() {
    // Hand-build a tiny archive whose manifest claims a different
    // lets-chat version; the gate should refuse before checksumming.
    let out = std::env::temp_dir().join(format!("lc-backup-ver-{}.zip", uuid::Uuid::new_v4()));
    {
        use std::io::Write;
        let f = std::fs::File::create(&out).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let m = backup::Manifest {
            version: "999.999.999".to_string(),
            git_hash: "deadbeef".to_string(),
            created_at: "2099-01-01T00:00:00Z".to_string(),
            files: vec![],
        };
        zw.start_file("manifest.json", opts).unwrap();
        zw.write_all(&serde_json::to_vec_pretty(&m).unwrap())
            .unwrap();
        zw.finish().unwrap();
    }
    let err = backup::verify_archive(&out).expect_err("version mismatch must be rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("999.999.999"),
        "rejection should mention the archive version; got: {msg}"
    );
    let _ = std::fs::remove_file(&out);
}

#[tokio::test]
async fn apply_pending_restore_swaps_when_marker_present() {
    // Self-contained tempdirs - no AppState needed.
    let root = std::env::temp_dir().join(format!("lc-restore-swap-{}", uuid::Uuid::new_v4()));
    let data_dir = root.join("data");
    let staged = backup::staged_dir_for(&data_dir);

    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("old.txt"), b"old contents").unwrap();
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("new.txt"), b"new contents").unwrap();
    std::fs::write(backup::marker_path_for(&data_dir), b"").unwrap();

    backup::apply_pending_restore(&data_dir).expect("swap should succeed");

    // data_dir now contains the staged contents.
    assert!(data_dir.join("new.txt").exists());
    assert!(!data_dir.join("old.txt").exists());
    // Staged dir is gone.
    assert!(!staged.exists());
    // Some `.replaced-*` sibling exists holding the old data.
    let parent = data_dir.parent().unwrap();
    let mut found_replaced = false;
    for entry in std::fs::read_dir(parent).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("data.replaced-") {
            assert!(entry.path().join("old.txt").exists());
            found_replaced = true;
            break;
        }
    }
    assert!(found_replaced, "old data must be preserved under a sibling");

    // Cleanup.
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn apply_pending_restore_clears_orphan_marker() {
    // Marker present but no staged dir: clear marker, no-op.
    let root = std::env::temp_dir().join(format!("lc-restore-orphan-{}", uuid::Uuid::new_v4()));
    let data_dir = root.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(backup::marker_path_for(&data_dir), b"").unwrap();

    backup::apply_pending_restore(&data_dir).unwrap();
    assert!(
        !backup::marker_path_for(&data_dir).exists(),
        "orphan marker should be removed"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ── HTTP routes ──────────────────────────────────────────────────────────

#[cfg(feature = "standalone")]
async fn send(
    app: &Router,
    sess: Option<&str>,
    method: Method,
    uri: &str,
    body: &str,
) -> StatusCode {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(s) = sess {
        builder = builder.header(header::COOKIE, format!("session={s}"));
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
        .status()
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_backup_endpoint_streams_a_valid_zip() {
    let t = app().await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/admin/backup")
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::empty())
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    let (status, bytes) = read_body(res).await;
    assert_eq!(status, StatusCode::OK);
    assert!(bytes.len() > 100, "archive should not be empty");
    // Verify by re-parsing the streamed bytes as a zip.
    let tmp = std::env::temp_dir().join(format!("lc-stream-{}.zip", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, &bytes).unwrap();
    let manifest = backup::verify_archive(&tmp).expect("streamed archive must verify");
    assert_eq!(manifest.version, lets_chat::version::VERSION);
    let _ = std::fs::remove_file(&tmp);
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn admin_restore_endpoint_stages_and_logs() {
    let t = app().await;
    // Produce a real archive first.
    let zip = std::env::temp_dir().join(format!("lc-restore-fixt-{}.zip", uuid::Uuid::new_v4()));
    backup::build_archive(
        &t.auth,
        &t.chat,
        &t.settings,
        &PathBuf::from(db::data_dir()),
        &zip,
    )
    .await
    .unwrap();
    let archive_bytes = std::fs::read(&zip).unwrap();

    // Build a minimal multipart body with one "archive" field.
    let boundary = "----lc-restore-boundary";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"archive\"; filename=\"backup.zip\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: application/zip\r\n\r\n");
    body.extend_from_slice(&archive_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method(Method::POST)
        .uri("/admin/restore")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .body(Body::from(body))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER, "stage should redirect");

    // Marker + staged dir should now exist.
    let data_dir = PathBuf::from(db::data_dir());
    assert!(backup::marker_path_for(&data_dir).exists());
    assert!(backup::staged_dir_for(&data_dir).exists());

    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    assert!(
        actions.iter().any(|a| a.action == "restore_stage"),
        "stage must audit-log"
    );

    // Clean up the staged dir + marker so the next test in this
    // process doesn't see a leftover swap.
    let _ = std::fs::remove_dir_all(backup::staged_dir_for(&data_dir));
    let _ = std::fs::remove_file(backup::marker_path_for(&data_dir));
    let _ = std::fs::remove_file(&zip);
}

#[cfg(feature = "standalone")]
#[tokio::test]
async fn non_admin_cannot_open_backup_pages() {
    let t = app().await;
    for path in ["/admin/backup-restore"] {
        assert_eq!(
            send(&t.app, Some(&t.member_session), Method::GET, path, "").await,
            StatusCode::FORBIDDEN,
            "path: {path}"
        );
    }
    for path in ["/admin/backup"] {
        assert_eq!(
            send(&t.app, Some(&t.member_session), Method::POST, path, "").await,
            StatusCode::FORBIDDEN,
            "path: {path}"
        );
    }
}
