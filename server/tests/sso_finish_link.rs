//! Integration tests for the POST /auth/sso/finish-link handler that
//! receives the password-confirmation from the link-required
//! interstitial. Covers HMAC verify, password verify, account-match
//! check, and the happy-path link+session-mint.
//!
//! The envelope is minted directly via `link_envelope::mint` rather
//! than walking the full callback flow first, so each test stays
//! focused on the finish-link surface.

use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::sso::link_envelope::{self, LinkPayload, ENVELOPE_TTL_SECONDS};
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use tower::ServiceExt;

const KEY: [u8; 32] = [9u8; 32];

fn hash_password(password: &str) -> String {
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::Argon2;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-finish-link-{}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

async fn open_pool(name: &str) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations: Vec<&str> = match name {
        "auth" => vec![
            include_str!("../migrations/auth/0001_create_tables.sql"),
            include_str!("../migrations/auth/0002_read_receipts.sql"),
            include_str!("../migrations/auth/0003_profile_fields.sql"),
            include_str!("../migrations/auth/0004_user_status.sql"),
            include_str!("../migrations/auth/0005_profile_visibility.sql"),
            include_str!("../migrations/auth/0006_user_blocks.sql"),
            include_str!("../migrations/auth/0007_notification_settings.sql"),
            include_str!("../migrations/auth/0008_two_factor.sql"),
            include_str!("../migrations/auth/0009_push_subscriptions.sql"),
            include_str!("../migrations/auth/0010_password_reset.sql"),
            include_str!("../migrations/auth/0011_email_verification.sql"),
            include_str!("../migrations/auth/0012_session_metadata.sql"),
            include_str!("../migrations/auth/0013_digest_columns.sql"),
            include_str!("../migrations/auth/0014_login_alerts.sql"),
            include_str!("../migrations/auth/0015_pending_registrations.sql"),
            include_str!("../migrations/auth/0016_sso_identities.sql"),
            include_str!("../migrations/auth/0017_sso_providers.sql"),
            include_str!("../migrations/auth/0018_sso_flows_provider.sql"),
        ],
        "chat" => vec![
            include_str!("../migrations/chat/0001_create_tables.sql"),
            include_str!("../migrations/chat/0002_moderation.sql"),
            include_str!("../migrations/chat/0003_dms.sql"),
            include_str!("../migrations/chat/0004_message_editing.sql"),
            include_str!("../migrations/chat/0005_private_rooms.sql"),
            include_str!("../migrations/chat/0006_read_receipts.sql"),
            include_str!("../migrations/chat/0007_reactions.sql"),
            include_str!("../migrations/chat/0008_search.sql"),
            include_str!("../migrations/chat/0009_enclaves.sql"),
            include_str!("../migrations/chat/0010_room_name_per_enclave.sql"),
            include_str!("../migrations/chat/0011_threads.sql"),
            include_str!("../migrations/chat/0012_uploads.sql"),
            include_str!("../migrations/chat/0013_link_previews.sql"),
            include_str!("../migrations/chat/0014_mentions.sql"),
            include_str!("../migrations/chat/0015_room_notification_settings.sql"),
            include_str!("../migrations/chat/0016_pinned_messages.sql"),
            include_str!("../migrations/chat/0017_custom_emojis.sql"),
            include_str!("../migrations/chat/0018_emoji_share_globally.sql"),
            include_str!("../migrations/chat/0019_bookmarks.sql"),
            include_str!("../migrations/chat/0020_quote_reply.sql"),
            include_str!("../migrations/chat/0021_enclave_invitations_enclave_idx.sql"),
            include_str!("../migrations/chat/0022_voice_messages.sql"),
            include_str!("../migrations/chat/0023_system_messages.sql"),
            include_str!("../migrations/chat/0024_voice_channel_flag.sql"),
        ],
        "settings" => vec![
            include_str!("../migrations/settings/0001_create_tables.sql"),
            include_str!("../migrations/settings/0002_uploads.sql"),
            include_str!("../migrations/settings/0003_vapid_keypair.sql"),
        ],
        _ => unreachable!(),
    };
    for sql in migrations {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn make_app() -> (Router, SqlitePool) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth: auth.clone(),
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new(KEY)),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://chat.example".to_string(),
        ice_servers: "[]".to_string(),
        sso: lets_chat::sso::SsoProviders::default(),
    };
    (routes::build_router(state), auth)
}

async fn seed_alice_with_password(auth: &SqlitePool) -> String {
    let id = db::auth::create_user(auth, "alice", &hash_password("hunter2"))
        .await
        .unwrap();
    db::auth::set_user_email(auth, &id, Some("alice@example.com"))
        .await
        .unwrap();
    id
}

fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn envelope_for(email: &str) -> String {
    link_envelope::mint(
        &KEY,
        &LinkPayload {
            provider_id: "stub".into(),
            issuer: "https://idp/".into(),
            subject: "sub-fresh".into(),
            email: email.to_string(),
            return_to: "/rooms/general".into(),
            not_after: now_unix() + ENVELOPE_TTL_SECONDS,
        },
    )
}

fn post(envelope: &str, username: &str, password: &str) -> Request<Body> {
    let body = format!(
        "envelope={}&username={}&password={}",
        urlencoding(envelope),
        urlencoding(username),
        urlencoding(password)
    );
    Request::builder()
        .method(Method::POST)
        .uri("/auth/sso/finish-link")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[tokio::test]
async fn happy_path_links_and_signs_in() {
    let (app, auth) = make_app().await;
    let alice = seed_alice_with_password(&auth).await;
    let envelope = envelope_for("alice@example.com");

    let res = app
        .clone()
        .oneshot(post(&envelope, "alice", "hunter2"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
        "/rooms/general"
    );
    // Session cookie set.
    assert!(res.headers().contains_key(header::SET_COOKIE));
    // sso_identities row written, with auto_linked=0 (explicit user action).
    let row: (i64, String) =
        sqlx::query_as("SELECT auto_linked, subject FROM sso_identities WHERE user_id = ?")
            .bind(&alice)
            .fetch_one(&auth)
            .await
            .unwrap();
    assert_eq!(row.0, 0);
    assert_eq!(row.1, "sub-fresh");
}

#[tokio::test]
async fn bad_hmac_rejected() {
    let (app, auth) = make_app().await;
    seed_alice_with_password(&auth).await;
    // Tamper: swap one char in the tag half.
    let mut env = envelope_for("alice@example.com");
    let tag_char_offset = env.find('.').unwrap() + 1;
    let mut bytes: Vec<u8> = env.bytes().collect();
    bytes[tag_char_offset] = if bytes[tag_char_offset] == b'A' {
        b'B'
    } else {
        b'A'
    };
    env = String::from_utf8(bytes).unwrap();

    let res = app
        .clone()
        .oneshot(post(&env, "alice", "hunter2"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    // No row written.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sso_identities")
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn wrong_password_rejected_even_with_valid_envelope() {
    let (app, auth) = make_app().await;
    seed_alice_with_password(&auth).await;
    let envelope = envelope_for("alice@example.com");

    let res = app
        .clone()
        .oneshot(post(&envelope, "alice", "WRONG"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sso_identities")
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn username_mismatch_with_envelope_email_rejected() {
    let (app, auth) = make_app().await;
    seed_alice_with_password(&auth).await;
    // Create a SECOND user with their own password but a different email.
    let bob = db::auth::create_user(&auth, "bob", &hash_password("hunter2"))
        .await
        .unwrap();
    db::auth::set_user_email(&auth, &bob, Some("bob@example.com"))
        .await
        .unwrap();
    // Envelope was minted for alice's email. Bob's correct password
    // must NOT splice into a link on alice's identity.
    let envelope = envelope_for("alice@example.com");
    let res = app
        .clone()
        .oneshot(post(&envelope, "bob", "hunter2"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn expired_envelope_rejected() {
    let (app, auth) = make_app().await;
    seed_alice_with_password(&auth).await;
    let envelope = link_envelope::mint(
        &KEY,
        &LinkPayload {
            provider_id: "stub".into(),
            issuer: "https://idp/".into(),
            subject: "sub-fresh".into(),
            email: "alice@example.com".into(),
            return_to: "/rooms/general".into(),
            not_after: now_unix() - 10,
        },
    );
    let res = app
        .clone()
        .oneshot(post(&envelope, "alice", "hunter2"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
