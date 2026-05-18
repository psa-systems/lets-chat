//! Integration tests for user-initiated link / unlink (doc 04 + 06):
//! POST /auth/sso/:provider/link starts an OIDC flow with kind=link;
//! the callback's link branch attaches the verified identity to the
//! signed-in user; POST /auth/sso/unlink removes the user's link
//! unless they have no local password.

use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use lets_chat::db::sso_providers::{self, InsertProvider};
use lets_chat::sso::secret;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use serde_json::json;
use sqlx::SqlitePool;
use tower::ServiceExt;

const SHARED_HS256_SECRET: &[u8] = b"test-shared-hs256-secret-bytes";
const KID: &str = "test-kid";

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-user-link-{}", std::process::id()));
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
            include_str!("../migrations/auth/0019_sso_group_mappings.sql"),
            include_str!("../migrations/auth/0020_session_tenant.sql"),
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
            include_str!("../migrations/chat/0025_message_edits.sql"),
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

struct Stub {
    issuer: String,
    nonce: Mutex<String>,
    sub: Mutex<String>,
}

async fn spawn_stub() -> Arc<Stub> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://127.0.0.1:{}", addr.port());
    let stub = Arc::new(Stub {
        issuer: issuer.clone(),
        nonce: Mutex::new(String::new()),
        sub: Mutex::new("sub-fresh".into()),
    });
    let discovery_body = format!(
        r#"{{
            "issuer": "{issuer}",
            "authorization_endpoint": "{issuer}/oauth2/authorize",
            "token_endpoint": "{issuer}/oauth2/token",
            "jwks_uri": "{issuer}/jwks.json"
        }}"#
    );
    let jwks_body = json!({
        "keys": [{
            "kty": "oct",
            "kid": KID,
            "alg": "HS256",
            "k": URL_SAFE_NO_PAD.encode(SHARED_HS256_SECRET),
        }]
    })
    .to_string();
    let stub_for_token = stub.clone();
    let issuer_for_token = issuer.clone();
    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let b = discovery_body.clone();
                async move { (StatusCode::OK, b) }
            }),
        )
        .route(
            "/jwks.json",
            get(move || {
                let b = jwks_body.clone();
                async move { (StatusCode::OK, b) }
            }),
        )
        .route(
            "/oauth2/token",
            post(move || {
                let stub = stub_for_token.clone();
                let issuer = issuer_for_token.clone();
                async move {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64;
                    let mut header = Header::new(Algorithm::HS256);
                    header.kid = Some(KID.into());
                    let claims = json!({
                        "iss": issuer,
                        "aud": "the-client-id",
                        "exp": now + 600,
                        "iat": now,
                        "sub": *stub.sub.lock().unwrap(),
                        "email": "alice@example.com",
                        "nonce": *stub.nonce.lock().unwrap(),
                    });
                    let id_token = encode(
                        &header,
                        &claims,
                        &EncodingKey::from_secret(SHARED_HS256_SECRET),
                    )
                    .unwrap();
                    let body = json!({
                        "id_token": id_token,
                        "token_type": "Bearer",
                    })
                    .to_string();
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "application/json")],
                        body,
                    )
                }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    stub
}

async fn make_app(stub: &Stub) -> (Router, SqlitePool) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let key = [9u8; 32];
    let enc = secret::encrypt_client_secret(&key, "secret").unwrap();
    sso_providers::insert_provider(
        &auth,
        InsertProvider {
            id: "stub",
            kind: "oidc",
            display_name: "Stub",
            issuer_url: &stub.issuer,
            client_id: "the-client-id",
            client_secret_encrypted: &enc,
            scopes: "openid email",
            attribute_map_json: "{}",
            allow_signup: false,
            auto_link_verified_email: true,
        },
    )
    .await
    .unwrap();
    sso_providers::set_provider_enabled(&auth, "stub", true)
        .await
        .unwrap();
    let sso = lets_chat::sso::SsoProviders::load_enabled(&auth)
        .await
        .unwrap();

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
        secret_key: Some(Arc::new(key)),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://chat.example".to_string(),
        ice_servers: "[]".to_string(),
        sso,
        local_login_disabled: false,
    };
    (routes::build_router(state), auth)
}

async fn sign_in_user(auth: &SqlitePool, username: &str) -> (String, String) {
    let id = db::auth::create_user(auth, username, "hash").await.unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&id)
        .execute(auth)
        .await
        .unwrap();
    let sess = db::auth::create_session(auth, &id).await.unwrap();
    (id, sess)
}

fn mkpost(uri: &str, sess: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method(Method::POST).uri(uri);
    if let Some(s) = sess {
        b = b.header(header::COOKIE, format!("session={s}"));
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn link_start_writes_link_flow_row_and_redirects() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    let (alice, sess) = sign_in_user(&auth, "alice").await;

    let res = app
        .clone()
        .oneshot(mkpost("/auth/sso/stub/link", Some(&sess)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(loc.starts_with(&format!("{}/oauth2/authorize", stub.issuer)));
    let parsed = url::Url::parse(loc).unwrap();
    let state_token = parsed
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .unwrap();
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT kind, user_id FROM sso_flows WHERE flow_id = ?")
            .bind(&state_token)
            .fetch_one(&auth)
            .await
            .unwrap();
    assert_eq!(row.0, "link");
    assert_eq!(row.1.as_deref(), Some(alice.as_str()));
}

#[tokio::test]
async fn link_start_requires_auth() {
    let stub = spawn_stub().await;
    let (app, _) = make_app(&stub).await;
    let res = app
        .clone()
        .oneshot(mkpost("/auth/sso/stub/link", None))
        .await
        .unwrap();
    // No session cookie => redirected to /login by middleware.
    assert!(
        res.status() == StatusCode::SEE_OTHER || res.status() == StatusCode::UNAUTHORIZED,
        "got {}",
        res.status()
    );
}

#[tokio::test]
async fn callback_link_branch_attaches_identity_to_the_signed_in_user() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    let (alice, sess) = sign_in_user(&auth, "alice").await;
    // Kick off the link flow to get a real state_token + nonce in
    // sso_flows pointing at alice.
    let res = app
        .clone()
        .oneshot(mkpost("/auth/sso/stub/link", Some(&sess)))
        .await
        .unwrap();
    let loc = res
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let parsed = url::Url::parse(&loc).unwrap();
    let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    let state_token = pairs.get("state").unwrap().clone();
    let nonce = pairs.get("nonce").unwrap().clone();
    *stub.nonce.lock().unwrap() = nonce;

    // Hit the callback. No session cookie required - the link branch
    // resolves the user from the sso_flows row, not the cookie.
    let cb = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/auth/sso/stub/callback?code=fake&state={state_token}"
        ))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(cb).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
        "/settings?sso_flash=linked"
    );
    // sso_identities row points at alice.
    let row: (String,) =
        sqlx::query_as("SELECT user_id FROM sso_identities WHERE subject = 'sub-fresh'")
            .fetch_one(&auth)
            .await
            .unwrap();
    assert_eq!(row.0, alice);
}

#[tokio::test]
async fn callback_link_refuses_when_identity_already_linked_elsewhere() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    // bob already owns the (issuer, "sub-fresh") link.
    let bob = db::auth::create_user(&auth, "bob", "hash").await.unwrap();
    db::sso::link_sso_identity(&auth, &bob, &stub.issuer, "sub-fresh", None, false)
        .await
        .unwrap();
    // alice tries to link the same identity.
    let (_alice, sess) = sign_in_user(&auth, "alice").await;
    let res = app
        .clone()
        .oneshot(mkpost("/auth/sso/stub/link", Some(&sess)))
        .await
        .unwrap();
    let loc = res
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let parsed = url::Url::parse(&loc).unwrap();
    let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    let state_token = pairs.get("state").unwrap().clone();
    let nonce = pairs.get("nonce").unwrap().clone();
    *stub.nonce.lock().unwrap() = nonce;

    let cb = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/auth/sso/stub/callback?code=fake&state={state_token}"
        ))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(cb).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
        "/settings?sso_flash=already_linked"
    );
    // Bob's row untouched.
    let owner: (String,) =
        sqlx::query_as("SELECT user_id FROM sso_identities WHERE subject = 'sub-fresh'")
            .fetch_one(&auth)
            .await
            .unwrap();
    assert_eq!(owner.0, bob);
}

#[tokio::test]
async fn user_unlink_removes_row_and_redirects() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    let (alice, sess) = sign_in_user(&auth, "alice").await;
    db::sso::link_sso_identity(&auth, &alice, &stub.issuer, "sub-x", None, false)
        .await
        .unwrap();
    let res = app
        .clone()
        .oneshot(mkpost("/auth/sso/unlink", Some(&sess)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
        "/settings?sso_flash=unlinked"
    );
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sso_identities WHERE user_id = ?")
        .bind(&alice)
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn user_unlink_refused_when_no_password() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    // SSO-only user: NULL password_hash.
    let uid = db::sso::create_user_from_sso(
        &auth,
        db::sso::CreateUserFromSso {
            issuer: &stub.issuer,
            subject: "sso-only-sub",
            email: Some("sso@example.com"),
            preferred_username: Some("ssouser"),
            display_name: None,
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&uid)
        .execute(&auth)
        .await
        .unwrap();
    let sess = db::auth::create_session(&auth, &uid).await.unwrap();

    let res = app
        .clone()
        .oneshot(mkpost("/auth/sso/unlink", Some(&sess)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
        "/settings?sso_flash=no_password"
    );
    // Identity row still present.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sso_identities WHERE user_id = ?")
        .bind(&uid)
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(count.0, 1);
}

#[tokio::test]
async fn settings_page_renders_linked_card_and_link_button() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    let (_alice, sess) = sign_in_user(&auth, "alice").await;
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/settings")
                .header(header::COOKIE, format!("session={sess}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains("Linked accounts"));
    assert!(body.contains("Link Stub to my account"));
    assert!(body.contains("/auth/sso/stub/link"));
}
