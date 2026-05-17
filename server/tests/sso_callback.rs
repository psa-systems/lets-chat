//! Integration tests for `/auth/sso/:provider/callback`. Stands up a
//! stub IdP that signs id_tokens with HS256 + serves the matching JWKS,
//! seeds a `sso_identities` row so the linked-user happy path applies,
//! and walks the full code-exchange + JWT-verify dance.
//!
//! Why HS256 in a test: lets us share one symmetric key between the
//! stub's signing path and the route's verify path without generating
//! an RSA keypair fixture. The production callback supports RS256 +
//! ES256 + HS256; this test exercises HS256 only.

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
        let p = std::env::temp_dir().join(format!("lets-chat-sso-cb-{}", std::process::id()));
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

struct StubState {
    issuer: String,
    /// Nonce echoed back into the id_token. Set by the test before
    /// hitting the `/token` endpoint.
    nonce: Mutex<String>,
    /// Subject echoed into id_token. Lets each test pick its own.
    sub: Mutex<String>,
    /// Audience echoed into id_token. Defaults to the registered
    /// client_id; can be set to a wrong value to exercise verify
    /// failure paths.
    audience: Mutex<String>,
    /// Email claim. Default points at alice; auto-link tests flip this.
    email: Mutex<String>,
    /// email_verified claim.
    email_verified: Mutex<bool>,
}

async fn spawn_stub() -> Arc<StubState> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://127.0.0.1:{}", addr.port());
    let stub = Arc::new(StubState {
        issuer: issuer.clone(),
        nonce: Mutex::new(String::new()),
        sub: Mutex::new("sub-default".into()),
        audience: Mutex::new("the-client-id".into()),
        email: Mutex::new("alice@example.com".into()),
        email_verified: Mutex::new(true),
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
                        "aud": *stub.audience.lock().unwrap(),
                        "exp": now + 600,
                        "iat": now,
                        "sub": *stub.sub.lock().unwrap(),
                        "email": *stub.email.lock().unwrap(),
                        "email_verified": *stub.email_verified.lock().unwrap(),
                        "name": "Alice",
                        "preferred_username": "alice",
                        "nonce": *stub.nonce.lock().unwrap(),
                    });
                    let id_token = encode(
                        &header,
                        &claims,
                        &EncodingKey::from_secret(SHARED_HS256_SECRET),
                    )
                    .unwrap();
                    let body = json!({
                        "access_token": "fake-access",
                        "id_token": id_token,
                        "token_type": "Bearer",
                        "expires_in": 600,
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

async fn make_app(stub: &StubState) -> (Router, SqlitePool) {
    make_app_with_flags(stub, true, false).await
}

async fn make_app_with_provider_flags(
    stub: &StubState,
    auto_link_verified_email: bool,
) -> (Router, SqlitePool) {
    make_app_with_flags(stub, auto_link_verified_email, false).await
}

async fn make_app_with_flags(
    stub: &StubState,
    auto_link_verified_email: bool,
    allow_signup: bool,
) -> (Router, SqlitePool) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    // Encrypt the (irrelevant in HS256 test) client secret so the
    // route's decrypt step works.
    let key = [9u8; 32];
    let enc = secret::encrypt_client_secret(&key, "client-secret").unwrap();
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
            allow_signup,
            auto_link_verified_email,
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

/// Drive the start route to mint a real sso_flows row + matching nonce.
/// Returns (state_token, nonce, pkce_verifier).
async fn start_flow(app: &Router, auth: &SqlitePool) -> (String, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/auth/sso/stub/start?return_to=/rooms/general")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
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

    // Sanity: a row exists in sso_flows for this state.
    let row: (String,) = sqlx::query_as("SELECT nonce FROM sso_flows WHERE flow_id = ?")
        .bind(&state_token)
        .fetch_one(auth)
        .await
        .unwrap();
    assert_eq!(row.0, nonce);

    (state_token, nonce)
}

fn cb_req(state_token: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/auth/sso/stub/callback?code=fake-code&state={state_token}"
        ))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn callback_signs_linked_user_in_and_redirects() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;

    // Pre-seed: a user with a sso_identities link to our (issuer, sub).
    let alice = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    db::sso::link_sso_identity(&auth, &alice, &stub.issuer, "sub-default", None, false)
        .await
        .unwrap();

    let (state_token, nonce) = start_flow(&app, &auth).await;
    *stub.nonce.lock().unwrap() = nonce;

    let res = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
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
    let set_cookie = res.headers().get(header::SET_COOKIE).unwrap();
    let s = set_cookie.to_str().unwrap();
    assert!(s.starts_with("session="), "session cookie present: {s}");
}

#[tokio::test]
async fn callback_rejects_state_for_wrong_provider() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    let (state_token, _) = start_flow(&app, &auth).await;

    // Hit /auth/sso/other/callback - the state was minted for "stub".
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/auth/sso/other/callback?code=c&state={state_token}"
        ))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    // "other" isn't even a registered provider -> 404 from lookup OR
    // 400 from the consume-then-mismatch path. Either is acceptable;
    // both mean "we refused."
    assert!(
        res.status() == StatusCode::NOT_FOUND || res.status() == StatusCode::BAD_REQUEST,
        "got {}",
        res.status()
    );
}

#[tokio::test]
async fn callback_rejects_expired_or_replayed_state() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    let (state_token, nonce) = start_flow(&app, &auth).await;
    *stub.nonce.lock().unwrap() = nonce;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    db::sso::link_sso_identity(&auth, &alice, &stub.issuer, "sub-default", None, false)
        .await
        .unwrap();

    // First callback succeeds.
    let res1 = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
    assert_eq!(res1.status(), StatusCode::SEE_OTHER);

    // Replay the same state: consumed already, must refuse.
    let res2 = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
    assert_eq!(res2.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn callback_rejects_idp_error_redirect() {
    let stub = spawn_stub().await;
    let (app, _auth) = make_app(&stub).await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/auth/sso/stub/callback?error=access_denied&error_description=user+cancelled")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn callback_auto_links_existing_user_on_verified_email() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;

    // Pre-seed an existing password user with the email the IdP will assert.
    let alice = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    db::auth::set_user_email(&auth, &alice, Some("alice@example.com"))
        .await
        .unwrap();

    let (state_token, nonce) = start_flow(&app, &auth).await;
    *stub.nonce.lock().unwrap() = nonce;
    // brand-new sub: not yet in sso_identities. email_verified stays true.
    *stub.sub.lock().unwrap() = "fresh-sub".into();

    let res = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
        "/rooms/general"
    );
    // Link row written with auto_linked = 1.
    let row: (i64,) = sqlx::query_as("SELECT auto_linked FROM sso_identities WHERE user_id = ?")
        .bind(&alice)
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(row.0, 1, "auto_linked flag set for the auto-link path");
}

#[tokio::test]
async fn callback_does_not_auto_link_when_email_unverified() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    let alice = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    db::auth::set_user_email(&auth, &alice, Some("alice@example.com"))
        .await
        .unwrap();

    let (state_token, nonce) = start_flow(&app, &auth).await;
    *stub.nonce.lock().unwrap() = nonce;
    *stub.sub.lock().unwrap() = "fresh-sub".into();
    *stub.email_verified.lock().unwrap() = false;

    let res = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
    // Renders the link-required interstitial (200 with the form).
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains("Confirm to link your account"));
    assert!(body.contains("name=\"envelope\""));
    // No sso_identities row written yet; the user has to POST the form.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sso_identities")
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn callback_does_not_auto_link_when_provider_flag_off() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app_with_provider_flags(&stub, false).await;
    let alice = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    db::auth::set_user_email(&auth, &alice, Some("alice@example.com"))
        .await
        .unwrap();

    let (state_token, nonce) = start_flow(&app, &auth).await;
    *stub.nonce.lock().unwrap() = nonce;
    *stub.sub.lock().unwrap() = "fresh-sub".into();
    // email_verified=true but provider's flag is off -> still goes
    // through the link-required interstitial.

    let res = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains("Confirm to link your account"));
}

#[tokio::test]
async fn callback_unknown_email_with_signup_off_renders_unauthorized_page() {
    let stub = spawn_stub().await;
    // allow_signup=false (the default in make_app) -> render the
    // "ask your admin" page, do NOT create a user.
    let (app, auth) = make_app(&stub).await;

    let (state_token, nonce) = start_flow(&app, &auth).await;
    *stub.nonce.lock().unwrap() = nonce;
    *stub.sub.lock().unwrap() = "fresh-sub".into();
    *stub.email.lock().unwrap() = "nobody@example.com".into();

    let res = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains("isn't authorized"));
    // No user created.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn callback_autoprovisions_when_allow_signup_and_email_verified() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app_with_flags(&stub, true, true).await;

    let (state_token, nonce) = start_flow(&app, &auth).await;
    *stub.nonce.lock().unwrap() = nonce;
    *stub.sub.lock().unwrap() = "fresh-sub".into();
    *stub.email.lock().unwrap() = "newbie@example.com".into();

    let res = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
        "/rooms/general"
    );
    // Fresh user row + sso_identities row land in the same transaction
    // via create_user_from_sso. password_hash is NULL.
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE email = ?")
            .bind("newbie@example.com")
            .fetch_one(&auth)
            .await
            .unwrap();
    assert!(
        row.1.is_none(),
        "autoprovisioned users have NULL password_hash"
    );
    let link: (String, i64) =
        sqlx::query_as("SELECT subject, auto_linked FROM sso_identities WHERE user_id = ?")
            .bind(&row.0)
            .fetch_one(&auth)
            .await
            .unwrap();
    assert_eq!(link.0, "fresh-sub");
    assert_eq!(link.1, 0, "auto_linked is for email-match path only");
}

#[tokio::test]
async fn callback_autoprovision_blocked_by_email_unverified() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app_with_flags(&stub, true, true).await;

    let (state_token, nonce) = start_flow(&app, &auth).await;
    *stub.nonce.lock().unwrap() = nonce;
    *stub.sub.lock().unwrap() = "fresh-sub".into();
    *stub.email.lock().unwrap() = "newbie@example.com".into();
    *stub.email_verified.lock().unwrap() = false;

    let res = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains("Verify your email first"));
    // No user created.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn callback_rejects_wrong_audience() {
    let stub = spawn_stub().await;
    let (app, auth) = make_app(&stub).await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    db::sso::link_sso_identity(&auth, &alice, &stub.issuer, "sub-default", None, false)
        .await
        .unwrap();
    let (state_token, nonce) = start_flow(&app, &auth).await;
    *stub.nonce.lock().unwrap() = nonce;
    *stub.audience.lock().unwrap() = "different-client".into();

    let res = app.clone().oneshot(cb_req(&state_token)).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
