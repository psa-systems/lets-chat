//! Integration tests for `/auth/sso/:provider/start`. Stands up a stub
//! IdP for discovery, registers a provider row, hits the route, and
//! checks: the 302 lands on the IdP's authorize endpoint with the
//! expected query params, the `sso_flows` row is persisted, and
//! unknown / disabled providers return 404.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::db::sso_providers::{self, InsertProvider};
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lets-chat-sso-start-{}", std::process::id()));
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

/// Spawn a tiny axum stub that answers /.well-known/openid-configuration.
async fn spawn_stub() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://127.0.0.1:{}", addr.port());
    let body = format!(
        r#"{{
            "issuer": "{issuer}",
            "authorization_endpoint": "{issuer}/oauth2/authorize",
            "token_endpoint": "{issuer}/oauth2/token",
            "jwks_uri": "{issuer}/jwks.json"
        }}"#
    );
    let app = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || {
                let b = body.clone();
                async move { (StatusCode::OK, b) }
            }),
        )
        .route(
            "/jwks.json",
            axum::routing::get(|| async { (StatusCode::OK, r#"{"keys":[]}"#) }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    issuer
}

async fn make_app_with_provider(
    issuer: &str,
    enabled: bool,
) -> (Router, SqlitePool, lets_chat::sso::SsoProviders) {
    ensure_tempdir();
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    sso_providers::insert_provider(
        &auth,
        InsertProvider {
            id: "stub",
            kind: "oidc",
            display_name: "Stub",
            issuer_url: issuer,
            client_id: "the-client-id",
            client_secret_encrypted: b"opaque",
            scopes: "openid email",
            attribute_map_json: "{}",
            allow_signup: false,
            auto_link_verified_email: true,
        },
    )
    .await
    .unwrap();
    if enabled {
        sso_providers::set_provider_enabled(&auth, "stub", true)
            .await
            .unwrap();
    }
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
        secret_key: Some(Arc::new([3u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
        mailer: None,
        base_url: "http://chat.example".to_string(),
        ice_servers: "[]".to_string(),
        sso: sso.clone(),
        local_login_disabled: false,
    };
    (routes::build_router(state), auth, sso)
}

fn req(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn start_redirects_to_authorize_with_expected_params() {
    let issuer = spawn_stub().await;
    let (app, auth, _) = make_app_with_provider(&issuer, true).await;

    let res = app
        .clone()
        .oneshot(req("/auth/sso/stub/start?return_to=/rooms/general"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        loc.starts_with(&format!("{issuer}/oauth2/authorize?")),
        "redirect goes to authorize endpoint: {loc}"
    );
    let parsed = url::Url::parse(&loc).unwrap();
    let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(pairs.get("response_type").map(|s| s.as_str()), Some("code"));
    assert_eq!(
        pairs.get("client_id").map(|s| s.as_str()),
        Some("the-client-id")
    );
    assert_eq!(
        pairs.get("redirect_uri").map(|s| s.as_str()),
        Some("http://chat.example/auth/sso/stub/callback")
    );
    assert_eq!(pairs.get("scope").map(|s| s.as_str()), Some("openid email"));
    assert_eq!(
        pairs.get("code_challenge_method").map(|s| s.as_str()),
        Some("S256")
    );
    assert!(pairs.contains_key("state"), "state present");
    assert!(pairs.contains_key("nonce"), "nonce present");
    assert!(
        pairs.contains_key("code_challenge"),
        "PKCE challenge present"
    );

    // sso_flows row exists with matching provider_id + return_to.
    let state_token = pairs.get("state").unwrap().clone();
    let row = sqlx::query("SELECT provider_id, return_to, kind FROM sso_flows WHERE flow_id = ?")
        .bind(&state_token)
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("provider_id"), "stub");
    assert_eq!(row.get::<String, _>("return_to"), "/rooms/general");
    assert_eq!(row.get::<String, _>("kind"), "sign_in");
}

#[tokio::test]
async fn unknown_provider_404s() {
    let issuer = spawn_stub().await;
    let (app, _, _) = make_app_with_provider(&issuer, true).await;
    let res = app
        .clone()
        .oneshot(req("/auth/sso/ghost/start"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn disabled_provider_is_not_in_cache_so_404s() {
    let issuer = spawn_stub().await;
    // Insert but never enable -> not in load_enabled cache.
    let (app, _, _) = make_app_with_provider(&issuer, false).await;
    let res = app
        .clone()
        .oneshot(req("/auth/sso/stub/start"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn open_redirect_smuggling_falls_back_to_root() {
    let issuer = spawn_stub().await;
    let (app, auth, _) = make_app_with_provider(&issuer, true).await;
    let res = app
        .clone()
        .oneshot(req(
            "/auth/sso/stub/start?return_to=https%3A%2F%2Fevil.example%2F",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    // The stored return_to in sso_flows must be `/`, not the smuggled URL.
    let row = sqlx::query("SELECT return_to FROM sso_flows ORDER BY rowid DESC LIMIT 1")
        .fetch_one(&auth)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("return_to"), "/");
}
