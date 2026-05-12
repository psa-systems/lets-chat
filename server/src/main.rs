use std::net::SocketAddr;

use lets_chat::{db, routes, state::AppState, version, ws::events::ChatEvent, ws::hub::Hub};

#[cfg(feature = "saas")]
const APP_NAME: &str = "lets-chat-saas";
#[cfg(not(feature = "saas"))]
const APP_NAME: &str = "lets-chat";

#[tokio::main]
async fn main() {
    if wants_version_flag() {
        println!("{}", version::banner(APP_NAME));
        return;
    }
    init_tracing();
    tracing::info!(
        app = APP_NAME,
        version = version::VERSION,
        git_version = version::GIT_VERSION,
        git_hash = version::GIT_HASH,
        build_date = version::BUILD_DATE,
        "build info"
    );
    let data_dir = parse_data_dir()
        .or_else(|| std::env::var("LETS_CHAT_DATA_DIR").ok())
        .unwrap_or_else(|| "/data".to_string());
    tracing::info!(%data_dir, "starting lets-chat");
    db::set_data_dir(data_dir);

    let auth_pool = db::open_auth_pool().await;
    let chat_pool = db::open_chat_pool().await;
    let settings_pool = db::open_settings_pool().await;
    let secret_key = lets_chat::crypto::load_secret_key_from_env().map(std::sync::Arc::new);

    // VAPID keypair: generate on first boot when a secret key is set, then
    // hold an `Arc` of the decrypted keypair for the lifetime of the process.
    // Without a secret key Push stays disabled (parallels the 2FA pattern).
    let vapid: Option<std::sync::Arc<lets_chat::db::vapid::VapidKeypair>> =
        match secret_key.as_ref() {
            Some(key) => {
                match lets_chat::db::vapid::load_or_generate(&settings_pool, key.as_ref()).await {
                    Ok(kp) => Some(std::sync::Arc::new(kp)),
                    Err(e) => {
                        tracing::warn!(error = %e, "vapid keypair load failed; push disabled");
                        None
                    }
                }
            }
            None => None,
        };
    let push_contact = std::env::var("LETS_CHAT_PUSH_CONTACT")
        .unwrap_or_else(|_| "mailto:admin@localhost".to_string());
    let push_client: std::sync::Arc<dyn lets_chat::push::PushClient> = match vapid.as_ref() {
        Some(kp) => std::sync::Arc::new(lets_chat::push::ReqwestPushClient::new(
            kp.clone(),
            push_contact,
        )),
        None => std::sync::Arc::new(lets_chat::push::ReqwestPushClient::new(
            std::sync::Arc::new(lets_chat::db::vapid::VapidKeypair {
                public_key_b64url: String::new(),
                private_key_bytes: vec![0u8; 32],
            }),
            push_contact,
        )),
    };

    let mailer = lets_chat::mail::Mailer::from_env();
    if mailer.is_some() {
        tracing::info!("SMTP mailer configured");
    } else {
        tracing::info!("SMTP mailer not configured; password reset disabled");
    }
    let base_url = std::env::var("LETS_CHAT_BASE_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://localhost:8080".to_string());

    let bg = lets_chat::bg::spawn(auth_pool.clone());
    let state = AppState {
        auth: auth_pool,
        chat: chat_pool,
        settings: settings_pool,
        hub: std::sync::Arc::new(Hub::new()),
        asset_version: compute_asset_version(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key,
        vapid,
        push_client,
        mailer,
        base_url,
    };

    if let Err(e) = db::enclave::backfill_general_membership(&state.auth, &state.chat).await {
        tracing::warn!(error = %e, "enclave backfill failed at startup");
    }

    // Eagerly load syntect's bundled syntax and theme sets on a blocking
    // thread before we start serving traffic. The deserialization takes
    // several seconds; doing it lazily inside the first markdown render
    // would freeze a tokio worker thread mid-request and starve any task
    // already scheduled on that thread.
    let warm = tokio::task::spawn_blocking(lets_chat::views::markdown::warm_syntect);
    let _ = warm.await;

    spawn_idle_scanner(state.clone());
    spawn_pool_stats_logger(state.clone());

    let app = routes::build_router(state);
    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let addr: SocketAddr = bind.parse().expect("invalid BIND_ADDR");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    tracing::info!(%addr, "listening");
    axum::serve(listener, app.into_make_service())
        .await
        .expect("server crashed");
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("lets_chat=info")),
        )
        .init();
}

/// Cache-buster for static assets. Uses the mtime of the built Tailwind CSS so
/// every rebuild forces browsers to re-fetch the stylesheet (and other vendored
/// assets that share the query). Falls back to a per-process random value if
/// the file cannot be stat'd, so cache busts are never silently disabled.
fn compute_asset_version() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let path = "server/assets/tailwind-built.css";
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(d) = modified.duration_since(UNIX_EPOCH) {
                return d.as_secs().to_string();
            }
        }
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

/// Periodically flip users from `'active'` to `'idle'` once their
/// `last_active_at` is older than 30 minutes. Runs every 60s. Each flip is
/// broadcast over the hub so other viewers can refresh status circles.
fn spawn_idle_scanner(state: AppState) {
    const IDLE_AFTER_SECS: i64 = 30 * 60;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.tick().await;
        loop {
            tick.tick().await;
            match db::auth::mark_idle_users(&state.auth, IDLE_AFTER_SECS).await {
                Ok(flipped) => {
                    for uid in flipped {
                        let custom = db::auth::find_user_by_id(&state.auth, &uid)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|r| r.custom_status);
                        state.hub.broadcast_global(&ChatEvent::UserStatusChanged {
                            user_id: uid,
                            status: db::auth::STATUS_IDLE.to_string(),
                            custom_status: custom,
                        });
                    }
                }
                Err(e) => tracing::warn!(error = %e, "idle scan failed"),
            }
        }
    });
}

/// Emit a tracing event every 30 s with current SQLite pool sizes for
/// auth/chat/settings. Use these numbers to confirm or rule out pool
/// exhaustion when investigating "page is slow to load" reports: if the
/// auth pool sits at `size == 16` with `idle == 0` for long stretches,
/// requests are queuing on `pool.acquire()`.
fn spawn_pool_stats_logger(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        tick.tick().await;
        loop {
            tick.tick().await;
            tracing::info!(
                auth_size = state.auth.size(),
                auth_idle = state.auth.num_idle(),
                chat_size = state.chat.size(),
                chat_idle = state.chat.num_idle(),
                settings_size = state.settings.size(),
                settings_idle = state.settings.num_idle(),
                "pool stats"
            );
        }
    });
}

fn parse_data_dir() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == "--data-dir")
        .map(|pair| pair[1].clone())
}

fn wants_version_flag() -> bool {
    std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
}
