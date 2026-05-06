use std::net::SocketAddr;

use lets_chat::{db, routes, state::AppState, ws::events::ChatEvent, ws::hub::Hub};

#[tokio::main]
async fn main() {
    init_tracing();
    let data_dir = parse_data_dir()
        .or_else(|| std::env::var("LETS_CHAT_DATA_DIR").ok())
        .unwrap_or_else(|| "/data".to_string());
    tracing::info!(%data_dir, "starting lets-chat");
    db::set_data_dir(data_dir);

    let state = AppState {
        auth: db::open_auth_pool().await,
        chat: db::open_chat_pool().await,
        settings: db::open_settings_pool().await,
        hub: std::sync::Arc::new(Hub::new()),
        asset_version: compute_asset_version(),
    };

    if let Err(e) = db::enclave::backfill_general_membership(&state.auth, &state.chat).await {
        tracing::warn!(error = %e, "enclave backfill failed at startup");
    }

    spawn_idle_scanner(state.clone());

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

fn parse_data_dir() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == "--data-dir")
        .map(|pair| pair[1].clone())
}
