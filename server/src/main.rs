use std::net::SocketAddr;

use lets_chat::{db, routes, state::AppState, version, ws::events::ChatEvent, ws::hub::Hub};

#[cfg(feature = "saas")]
const APP_NAME: &str = "lets-chat-saas";
#[cfg(not(feature = "saas"))]
const APP_NAME: &str = "lets-chat";

// Use a generous worker pool: the chat template render path is fully
// synchronous CPU work (pulldown_cmark + syntect), and even with the
// markdown cache and `block_in_place` wrap, a burst of cache-miss
// renders across concurrent broadcasts can pin every worker. Default
// `worker_threads = num_cpus` (often 4 in a container) leaves no spare
// thread to poll fresh /ws or HTTP upgrades, which manifested as
// multi-minute "page is loading" stalls. 32 is well above the steady-
// state need and costs only a handful of stack pages.
#[tokio::main(flavor = "multi_thread", worker_threads = 32)]
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
        tracing::info!("SMTP mailer not configured; password reset and digest disabled");
    }
    let base_url = std::env::var("LETS_CHAT_BASE_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://localhost:8080".to_string());

    let ice_servers = std::env::var("LETS_CHAT_ICE_SERVERS")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| r#"[{"urls":"stun:stun.l.google.com:19302"}]"#.to_string());

    let bg = lets_chat::bg::spawn(auth_pool.clone());

    // SSO providers. Live state is the `sso_providers` table; the
    // legacy LETS_CHAT_SSO_* env vars seed one default row on first
    // boot for the upgrade path from L2's env-var single-provider
    // config. Subsequent edits go through /admin/sso and the env vars
    // are silently honoured by the DB row (DB wins).
    match lets_chat::sso::SsoConfig::from_env() {
        Ok(Some(cfg)) => {
            let key = secret_key.as_ref().map(|k| k.as_ref());
            match lets_chat::sso::seed::seed_default_from_env(&auth_pool, key, &cfg).await {
                Ok(true) => tracing::info!(
                    issuer = %cfg.issuer,
                    autoprovision = cfg.autoprovision,
                    "seeded sso_providers row from env vars"
                ),
                Ok(false) => {}
                Err(e) => panic!("SSO seed failed: {e}"),
            }
        }
        Ok(None) => {
            tracing::info!("LETS_CHAT_SSO_ISSUER not set; relying on /admin/sso for providers")
        }
        Err(e) => panic!("SSO config error: {e}"),
    }
    let sso = match lets_chat::sso::SsoProviders::load_enabled(&auth_pool).await {
        Ok(p) => p,
        Err(e) => panic!("failed to load sso_providers: {e}"),
    };

    let local_login_disabled = parse_bool_env("LETS_CHAT_LOCAL_LOGIN_DISABLED", false);
    if local_login_disabled {
        tracing::info!(
            "LETS_CHAT_LOCAL_LOGIN_DISABLED=true; password sign-in, register, password reset, and /settings/password will 404"
        );
    }

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
        ice_servers,
        sso,
        local_login_disabled,
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
    spawn_digest_sender(state.clone());
    spawn_orphan_sweeper(state.clone());
    spawn_sso_flow_pruner(state.clone());

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

/// Hourly tick that calls `digest::run_tick`. Internal short-circuit
/// when `state.mailer` is `None` makes this safe to spawn even on
/// deployments without SMTP configured. Modeled on `spawn_idle_scanner`.
fn spawn_digest_sender(state: AppState) {
    let cfg = lets_chat::digest::DigestConfig::default();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        // Skip the immediate fire: tokio::time::interval fires once at
        // creation time. We want the first run an hour after boot so the
        // server is fully up and any startup hiccups have settled.
        tick.tick().await;
        loop {
            tick.tick().await;
            if let Err(e) = lets_chat::digest::run_tick(&state, cfg).await {
                tracing::warn!(error = %e, "digest tick failed");
            }
        }
    });
}

/// Hourly tick that calls `uploads::sweep::run_orphan_sweep` with the
/// 24-hour threshold. Modeled on `spawn_digest_sender`. The sweep itself
/// short-circuits cheaply when there are no candidates, so this stays quiet
/// on idle deployments.
fn spawn_orphan_sweeper(state: AppState) {
    const TICK_SECS: u64 = 3600;
    const THRESHOLD_HOURS: i64 = 24;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::uploads::sweep::run_orphan_sweep(&state.chat, THRESHOLD_HOURS).await {
                Ok(stats) if stats.rows_deleted > 0 || stats.errors > 0 => {
                    tracing::info!(
                        rows = stats.rows_deleted,
                        files = stats.files_deleted,
                        errors = stats.errors,
                        "orphan sweep complete",
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "orphan sweep failed"),
            }
        }
    });
}

/// Drop expired `sso_flows` rows every 5 minutes. The callback path
/// consumes each row on hit (DELETE ... RETURNING) so the only rows
/// left around are flows that were never finished - the user closed
/// the IdP tab, hit Cancel, or the network dropped. Without a pruner
/// these accumulate forever. The TTL is 10 minutes per doc 02 section
/// 12; a 5-minute tick keeps the table bounded.
fn spawn_sso_flow_pruner(state: AppState) {
    const TICK_SECS: u64 = 300;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match db::sso::prune_expired_sso_flows(&state.auth).await {
                Ok(n) if n > 0 => tracing::info!(removed = n, "sso_flows prune"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "sso_flows prune failed"),
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

fn parse_bool_env(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(s) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

fn wants_version_flag() -> bool {
    std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
}
