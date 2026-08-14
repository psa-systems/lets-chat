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
    // LC-77: rustls 0.23 requires a process-wide CryptoProvider before any
    // TLS connect. The IMAP poll loop (and any future rustls consumer in
    // this binary) panics on first use without this call. install_default
    // returns Err if a provider is already installed; we treat that as a
    // benign no-op since reqwest may install ring transitively in some
    // builds and we just need SOMETHING installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
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
    // LC-95: if the previous run staged a restore archive, swap the
    // staging directory into place before any pool opens against the
    // (about-to-be-replaced) data dir. A hard-stop on swap failure
    // is the right call: half-renamed state would surface as cryptic
    // pool errors later, and the operator's recovery is to rename
    // the sibling `.replaced-{ts}` directory back.
    if let Err(e) = lets_chat::backup::apply_pending_restore(std::path::Path::new(&data_dir)) {
        tracing::error!(error = %e, "failed to apply pending restore; refusing to start");
        std::process::exit(1);
    }
    db::set_data_dir(data_dir);

    let auth_pool = db::open_auth_pool().await;
    let chat_pool = db::open_chat_pool().await;
    // LC-621: ensure every pre-existing user is a member of the General default
    // enclave on each boot (idempotent INSERT OR IGNORE). The SSO signup path
    // auto-joins new users; only this backfill covers accounts that existed
    // before LC-621 (or before they next sign in after the upgrade).
    if let Err(e) = db::enclave::backfill_general_membership(&auth_pool, &chat_pool).await {
        tracing::warn!(error = %e, "General enclave backfill failed");
    }
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

    // LC-22 pure-RP cutover: load and initialize the Bunyip SSO client at
    // startup. The four `LETS_CHAT_BUNYIP_SSO_*` env vars are mandatory;
    // bunyip-api must be reachable for discovery + JWKS. Missing env or
    // unreachable OP is a hard startup failure - there is no "off" state and
    // no fallback path. See
    // `docs/lets-chat/sso/bunyip-only/04-lets-chat-server-cutover.md` §4.2.
    let bunyip_sso_cfg = match lets_chat::oidc::BunyipSsoConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "bunyip SSO config invalid; refusing to start");
            std::process::exit(1);
        }
    };
    let bunyip_sso = match lets_chat::oidc::BunyipSsoClient::initialize(bunyip_sso_cfg).await {
        Ok(c) => {
            tracing::info!(target: "bunyip_sso", "Bunyip RP initialized");
            Some(c)
        }
        Err(e) => {
            tracing::error!(error = %e, "bunyip SSO initialize failed; refusing to start");
            std::process::exit(1);
        }
    };

    // LC-393 Phase 3: optional server-side STT. Enabled by LETS_CHAT_STT_URL;
    // when unset, transcription stays on the in-browser Web Speech engine.
    let stt_client: Option<std::sync::Arc<dyn lets_chat::stt::SttClient>> =
        match lets_chat::stt::SttConfig::from_env() {
            Some(cfg) => {
                tracing::info!(target: "stt", url = %cfg.url, model = %cfg.model, "server-side STT enabled");
                Some(std::sync::Arc::new(lets_chat::stt::ReqwestSttClient::new(
                    cfg,
                )))
            }
            None => None,
        };

    // LC-396: optional LLM for transcript summaries. Enabled by LETS_CHAT_LLM_URL.
    let llm_client: Option<std::sync::Arc<dyn lets_chat::llm::LlmClient>> =
        match lets_chat::llm::LlmConfig::from_env() {
            Some(cfg) => {
                tracing::info!(target: "llm", url = %cfg.url, model = %cfg.model, "transcript summaries enabled");
                Some(std::sync::Arc::new(lets_chat::llm::ReqwestLlmClient::new(
                    cfg,
                )))
            }
            None => None,
        };

    // LC-549: optional text-embeddings endpoint for semantic / related-message
    // search. Enabled by LETS_CHAT_EMBEDDINGS_URL; absent = FTS-only search.
    let embedding_client: Option<std::sync::Arc<dyn lets_chat::embeddings::EmbeddingClient>> =
        match lets_chat::embeddings::EmbeddingsConfig::from_env() {
            Some(cfg) => {
                tracing::info!(target: "embeddings", url = %cfg.url, model = %cfg.model, "semantic search enabled");
                Some(std::sync::Arc::new(
                    lets_chat::embeddings::ReqwestEmbeddingClient::new(cfg),
                ))
            }
            None => None,
        };

    let bg = lets_chat::bg::spawn(auth_pool.clone());
    // LC-580: optional IP -> country resolver for login-location alerts.
    // `None` (env unset or .BIN load failure) leaves the feature disabled.
    let geoip = lets_chat::geoip::GeoipResolver::from_env().map(std::sync::Arc::new);
    // LC-587: opt-in suspicious-login approval gate. Off by default; any of
    // `1` / `true` / `yes` (case-insensitive) turns it on for this deployment.
    let login_approval_enabled = std::env::var("LOGIN_APPROVAL_ENABLED")
        .ok()
        .map(|v| {
            let v = v.trim();
            v.eq_ignore_ascii_case("1")
                || v.eq_ignore_ascii_case("true")
                || v.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false);
    if login_approval_enabled {
        tracing::info!(target: "login_approval", "suspicious-login approval gate ENABLED (LC-587)");
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
        // LC-91: mobile push senders are not wired yet (no native client +
        // no operator credentials). Device tokens are still accepted and
        // stored; delivery begins once these become `Some`.
        apns_client: None,
        fcm_client: None,
        mailer,
        geoip,
        login_approval_enabled,
        base_url,
        ice_servers,
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso,
        stt_client,
        llm_client,
        embedding_client,
    };

    // Eagerly load syntect's bundled syntax and theme sets on a blocking
    // thread before we start serving traffic. The deserialization takes
    // several seconds; doing it lazily inside the first markdown render
    // would freeze a tokio worker thread mid-request and starve any task
    // already scheduled on that thread.
    let warm = tokio::task::spawn_blocking(lets_chat::views::markdown::warm_syntect);
    let _ = warm.await;

    spawn_idle_scanner(state.clone());
    spawn_status_expiry_scanner(state.clone());
    spawn_mute_expiry_scanner(state.clone());
    spawn_digest_sender(state.clone());
    spawn_orphan_sweeper(state.clone());
    spawn_scheduled_dispatcher(state.clone());
    spawn_reminders_dispatcher(state.clone());
    spawn_polls_closer(state.clone());
    spawn_outgoing_webhook_dispatcher(state.clone());
    spawn_room_digest_dispatcher(state.clone());
    spawn_embedding_backfill_dispatcher(state.clone());
    spawn_help_docs_refresh_dispatcher(state.clone());
    spawn_analytics_aggregator(state.clone());
    spawn_weekly_recap_dispatcher(state.clone());
    spawn_message_retention_sweeper(state.clone());
    spawn_ephemeral_sweeper(state.clone());
    lets_chat::email_ingress::poll::spawn_email_poll(state.clone());

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

/// LC-319: periodically clear custom statuses whose `custom_status_expires_at`
/// has passed. Runs every 60s, mirroring `spawn_idle_scanner`. Each cleared row
/// is broadcast over the hub (presence value unchanged, custom text now None)
/// so other viewers' avatars/hovercards drop the stale text live. The 60s
/// granularity matches the idle-flip tolerance.
fn spawn_status_expiry_scanner(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.tick().await;
        loop {
            tick.tick().await;
            match db::auth::clear_expired_custom_statuses(&state.auth).await {
                Ok(cleared) => {
                    for (uid, status) in cleared {
                        state.hub.broadcast_global(&ChatEvent::UserStatusChanged {
                            user_id: uid,
                            status,
                            custom_status: None,
                        });
                    }
                }
                Err(e) => tracing::warn!(error = %e, "custom status expiry scan failed"),
            }
        }
    });
}

/// LC-535: every 60s, null out timed mutes whose `muted_until` has passed so
/// the admin table and exports stop reporting an expired mute. Enforcement at
/// the posting gates already honours expiry (`User::mute_in_effect`); this is
/// purely DB truthfulness. Modeled on `spawn_status_expiry_scanner`; the 60s
/// granularity matches the coarsest useful mute duration step.
fn spawn_mute_expiry_scanner(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.tick().await;
        loop {
            tick.tick().await;
            match db::auth::clear_expired_mutes(&state.auth).await {
                Ok(0) => {}
                Ok(n) => tracing::debug!(cleared = n, "expired mutes cleared"),
                Err(e) => tracing::warn!(error = %e, "mute expiry scan failed"),
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
/// LC-97: daily analytics aggregator. On startup it backfills every day
/// since the install's earliest activity (so a fresh deploy has its full
/// history immediately), then ticks hourly to keep the current day fresh.
/// Past days are immutable enough that an hourly recompute of today alone
/// is sufficient; admins can force a full day recompute from the
/// dashboard.
fn spawn_analytics_aggregator(state: AppState) {
    const TICK_SECS: u64 = 3600;
    tokio::spawn(async move {
        match sqlx::query_scalar::<_, String>("SELECT date('now')")
            .fetch_one(&state.chat)
            .await
        {
            Ok(today) => {
                match lets_chat::db::analytics::backfill(&state.auth, &state.chat, &today).await {
                    Ok(n) => tracing::info!(days = n, "analytics backfill complete"),
                    Err(e) => tracing::warn!(error = %e, "analytics backfill failed"),
                }
            }
            Err(e) => tracing::warn!(error = %e, "analytics backfill skipped (date query failed)"),
        }
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            let today: String = match sqlx::query_scalar("SELECT date('now')")
                .fetch_one(&state.chat)
                .await
            {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(error = %e, "analytics tick: date query failed");
                    continue;
                }
            };
            if let Err(e) =
                lets_chat::db::analytics::recompute_day(&state.auth, &state.chat, &today).await
            {
                tracing::warn!(error = %e, "analytics recompute failed");
            }
        }
    });
}

/// LC-63: poll the reminders table and fire due reminders. Mirrors the
/// scheduled-message dispatcher: a 30s tick, claim-then-notify, errors
/// logged and retried next tick.
fn spawn_reminders_dispatcher(state: AppState) {
    const TICK_SECS: u64 = 30;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::reminders::run_reminder_tick(&state).await {
                Ok(stats) if stats.fired > 0 => {
                    tracing::info!(fired = stats.fired, "reminder dispatch tick complete");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "reminder dispatch tick failed"),
            }
        }
    });
}

/// LC-75: deliver due outgoing-webhook deliveries. A 10s tick POSTs the
/// signed payload with retries + backoff; one shared reqwest client is built
/// once and reused. The URL/secret are never logged.
fn spawn_outgoing_webhook_dispatcher(state: AppState) {
    const TICK_SECS: u64 = 10;
    tokio::spawn(async move {
        // LC-152: each delivery's reqwest::Client is reached via
        // `http_client::outbound_post`, which applies the two-layer SSRF
        // guard (URL-input check for literal IPs + PublicOnlyResolver for
        // hostname TOCTOU). No client construction at the call site; no
        // client construction is reachable from `server/src/` outside the
        // helper, by structural design + the grep-ban test.
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::outgoing::run_delivery_tick(&state.chat).await {
                Ok(stats) if stats.delivered + stats.retried + stats.failed > 0 => {
                    tracing::info!(
                        delivered = stats.delivered,
                        retried = stats.retried,
                        failed = stats.failed,
                        "outgoing webhook delivery tick complete"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "outgoing webhook delivery tick failed"),
            }
        }
    });
}

/// LC-665: post scheduled per-room AI activity digests. A slow tick (30 min)
/// posts a once-per-day digest for each opted-in room via the assistant bot;
/// the once-per-interval + dedupe logic lives in `room_digest::run_digest_tick`.
fn spawn_room_digest_dispatcher(state: AppState) {
    const TICK_SECS: u64 = 1800;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::room_digest::run_digest_tick(&state).await {
                Ok(stats) if stats.posted > 0 => {
                    tracing::info!(
                        posted = stats.posted,
                        evaluated = stats.evaluated,
                        "room digest tick complete"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "room digest tick failed"),
            }
        }
    });
}

/// LC-671: personal weekly recap DM. Operator opt-in (`LETS_CHAT_WEEKLY_RECAP`,
/// off by default), so the dispatcher is not even spawned unless it is set. A
/// slow tick (6h) DMs each active user a short AI recap of their week at most
/// once per 7 days; the dedupe + eligibility live in `weekly_recap`.
fn spawn_weekly_recap_dispatcher(state: AppState) {
    let enabled = matches!(
        std::env::var("LETS_CHAT_WEEKLY_RECAP").ok().as_deref(),
        Some("1" | "true" | "on" | "yes")
    );
    if !enabled {
        return;
    }
    const TICK_SECS: u64 = 6 * 3600;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::weekly_recap::run_weekly_recap_tick(&state).await {
                Ok(stats) if stats.sent > 0 => {
                    tracing::info!(
                        sent = stats.sent,
                        evaluated = stats.evaluated,
                        "weekly recap tick complete"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "weekly recap tick failed"),
            }
        }
    });
}

/// LC-673: backfill embeddings for messages posted before the embeddings
/// endpoint was configured, so semantic search covers history and not just
/// messages sent after it was turned on. A slow tick embeds a small batch per
/// pass; a no-op once the backlog is drained or when no embeddings endpoint is
/// set. The per-message embed is best-effort (a transient endpoint error leaves
/// the message for a later tick).
fn spawn_embedding_backfill_dispatcher(state: AppState) {
    const TICK_SECS: u64 = 60;
    const BATCH: i64 = 50;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::routes::related::run_embedding_backfill_tick(&state, BATCH).await {
                Ok(n) if n > 0 => {
                    tracing::info!(embedded = n, "embedding backfill batch complete");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "embedding backfill tick failed"),
            }
        }
    });
}

/// LC-712: refresh the AI help desk documentation knowledge base. A slow tick
/// re-fetches the configured docs sources and re-embeds any page whose content
/// changed (content-hash skips unchanged pages). A no-op when the AI flag is off,
/// no embeddings endpoint is configured, or no sources are set. Errors are
/// best-effort (logged, the next tick retries), so a transient docs-host outage
/// never crashes the process.
fn spawn_help_docs_refresh_dispatcher(state: AppState) {
    // Six hours: docs change rarely, and an admin can force an immediate reindex
    // from the settings page.
    const TICK_SECS: u64 = 6 * 60 * 60;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        // Skip the immediate fire so startup does not hit the docs hosts.
        tick.tick().await;
        loop {
            tick.tick().await;
            if let Err(e) = lets_chat::routes::help_docs::run_refresh_tick(&state).await {
                tracing::warn!(error = %e, "help docs refresh tick failed");
            }
        }
    });
}

/// LC-66: close polls whose deadline has passed and broadcast a re-render
/// so the UI flips to results-only. 30s tick like the other dispatchers.
fn spawn_polls_closer(state: AppState) {
    use lets_chat::ws::events::ChatEvent;
    const TICK_SECS: u64 = 30;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::db::polls::close_due(&state.chat).await {
                Ok(closed) => {
                    for (message_id, room_id) in closed {
                        state.hub.broadcast_to_room(
                            room_id,
                            &ChatEvent::PollUpdated {
                                message_id,
                                room_id,
                            },
                        );
                    }
                }
                Err(e) => tracing::warn!(error = %e, "poll closer tick failed"),
            }
        }
    });
}

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
            // LC-77-REPLY: expired reply-by-email tokens piggyback on the
            // hourly orphan tick. The sweep is a single DELETE keyed on the
            // expires_at index; an idle deployment (no tokens issued)
            // touches zero rows.
            match lets_chat::db::reply_tokens::sweep_expired(&state.chat).await {
                Ok(n) if n > 0 => {
                    tracing::info!(rows = n, "reply-token sweep complete");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "reply-token sweep failed"),
            }
            // LC-77-MID-DEDUP: drop processed-Message-ID dedup rows
            // older than 30 days. A sender re-issuing the same Message-ID
            // after that window is treated as a fresh message; this
            // matches typical mail-system Message-ID lifetimes (most
            // senders include a timestamp + nonce that never recurs).
            match lets_chat::db::email_ingress_dedup::sweep_old(&state.chat, 30).await {
                Ok(n) if n > 0 => {
                    tracing::info!(rows = n, "email-ingress dedup sweep complete");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "email-ingress dedup sweep failed"),
            }
            // LC-207-OBSERVABILITY: drop the admin-visible email-ingress drop
            // log at the same 30-day horizon as the dedup table.
            match lets_chat::db::email_ingress_drops::sweep_old(&state.chat, 30).await {
                Ok(n) if n > 0 => {
                    tracing::info!(rows = n, "email-ingress drop-log sweep complete");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "email-ingress drop-log sweep failed"),
            }
            // LC-78-AVATAR-PROXY: drop unreferenced bridge-avatar cache
            // entries older than 30 days, and recover any pending-fetch
            // rows orphaned by a process restart (older than 10 minutes).
            let avatar_stats = lets_chat::bridge_avatar::sweep_tick(&state.chat, 30, 600).await;
            if avatar_stats.rows_deleted > 0
                || avatar_stats.pending_marked_failed > 0
                || avatar_stats.errors > 0
            {
                tracing::info!(
                    rows = avatar_stats.rows_deleted,
                    files = avatar_stats.files_deleted,
                    pending_failed = avatar_stats.pending_marked_failed,
                    errors = avatar_stats.errors,
                    "bridge avatar sweep complete",
                );
            }
        }
    });
}

/// 30-second tick that calls `scheduled::run_dispatch_tick`. Modeled on
/// `spawn_idle_scanner`. UI promises delivery within 30 seconds of the
/// scheduled time. Logs only on non-empty ticks so an idle deployment
/// stays quiet; warns on tick-level errors (per-row errors are absorbed
/// inside the tick and surface as bumped `attempt_count`).
fn spawn_scheduled_dispatcher(state: AppState) {
    const TICK_SECS: u64 = 30;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::scheduled::run_dispatch_tick(&state).await {
                Ok(stats) if stats.delivered > 0 || stats.dropped > 0 || stats.retries > 0 => {
                    tracing::info!(
                        delivered = stats.delivered,
                        dropped = stats.dropped,
                        retries = stats.retries,
                        "scheduled dispatch tick complete",
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "scheduled dispatch tick failed"),
            }
        }
    });
}

/// 1-hour tick that calls `retention::sweep::sweep_once`. Mirrors
/// `spawn_orphan_sweeper`'s shape (sibling slow-clock sweep) and skips
/// the first tick so the first sweep does not fire during the cold-start
/// window when other background tasks are warming up.
///
/// The env flag is checked at spawn time, not per-tick: if
/// `LETS_CHAT_RETENTION_SWEEP_ENABLED` is unset the task is never spawned
/// at all. Flipping the flag requires a server restart, which is the
/// right shape for a destructive-feature gate the operator is opting
/// into deliberately. The `run_retention_sweep` wrapper still exists for
/// callers that want the flag check inline (e.g., a future admin
/// "purge now" handler).
///
/// Broadcasting happens AFTER the transaction commits, outside the
/// `BEGIN IMMEDIATE` critical section, so channel writes do not block
/// the writer lock. Per-room broadcast goes only to currently-subscribed
/// clients; clients without the deleted message in their DOM ignore the
/// fragment silently. The "skip broadcast for messages too old for
/// anyone to be viewing" optimization is intentionally deferred: at
/// `SWEEP_LIMIT = 500` per hour and a ~50-byte fragment per delete, the
/// volume is negligible by chat-server standards. Revisit if metrics
/// surface pressure on the hub.
fn spawn_message_retention_sweeper(state: AppState) {
    if !lets_chat::retention::sweep::flag_enabled() {
        tracing::info!(
            "retention sweep disabled (LETS_CHAT_RETENTION_SWEEP_ENABLED unset; set it and restart to enable)",
        );
        return;
    }
    const TICK_SECS: u64 = 3600;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::retention::sweep::sweep_once(&state.chat).await {
                Ok(stats) => {
                    if stats.messages_deleted > 0 {
                        tracing::info!(
                            rooms = stats.rooms_touched,
                            messages = stats.messages_deleted,
                            "retention sweep complete",
                        );
                        for (message_id, room_id) in &stats.purged {
                            let event = lets_chat::ws::events::ChatEvent::MessagePurged {
                                message_id: *message_id,
                                room_id: *room_id,
                            };
                            state.hub.broadcast_to_room(*room_id, &event);
                        }
                    }
                    // LC-207-OBSERVABILITY (#278): record every completed tick
                    // (including zero-delete) so an operator who enabled the
                    // destructive sweep can confirm from the admin page that it
                    // ran and how much it deleted, without container logs.
                    if let Err(e) = lets_chat::db::retention_status::record_run(
                        &state.chat,
                        stats.rooms_touched as i64,
                        stats.messages_deleted as i64,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "retention sweep status write failed");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "retention sweep failed");
                    if let Err(e2) =
                        lets_chat::db::retention_status::record_error(&state.chat, &e.to_string())
                            .await
                    {
                        tracing::warn!(error = %e2, "retention sweep status write failed");
                    }
                }
            }
        }
    });
}

/// LC-547: unconditional 60-second sweep that hard-deletes messages whose
/// self-destruct time (`expires_at`) has passed and broadcasts `MessagePurged`
/// so connected clients drop the node from the DOM. Unlike
/// `spawn_message_retention_sweeper`, this is never env-gated: a per-message TTL
/// is the sender's intent, not an operator policy, so it always runs. The
/// 60-second cadence bounds how long an expired message lingers server-side (a
/// 5-minute timer disappears within a minute of expiry). Skips the first tick
/// so it does not fire during cold start, mirroring the sibling sweepers.
/// Broadcasting happens after the transaction commits (see
/// `sweep_expired_ephemeral`), keeping channel writes out of the writer-lock
/// critical section.
fn spawn_ephemeral_sweeper(state: AppState) {
    const TICK_SECS: u64 = 60;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;
        loop {
            tick.tick().await;
            match lets_chat::retention::sweep::sweep_expired_ephemeral(&state.chat).await {
                Ok(stats) => {
                    for (message_id, room_id) in &stats.purged {
                        let event = lets_chat::ws::events::ChatEvent::MessagePurged {
                            message_id: *message_id,
                            room_id: *room_id,
                        };
                        state.hub.broadcast_to_room(*room_id, &event);
                    }
                    if stats.messages_deleted > 0 {
                        tracing::info!(
                            rooms = stats.rooms_touched,
                            messages = stats.messages_deleted,
                            "ephemeral sweep complete",
                        );
                    }
                }
                Err(e) => tracing::warn!(error = %e, "ephemeral sweep failed"),
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

fn wants_version_flag() -> bool {
    std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
}
