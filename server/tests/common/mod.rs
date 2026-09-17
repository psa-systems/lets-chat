//! Shared test helpers.
//!
//! Integration tests need in-memory SQLite pools with the schema applied.
//! Each test file used to hand-list `include_str!` migrations, which silently
//! went stale every time a migration was added (e.g. `messages.quote_id`).
//! These helpers run the full embedded migration set via `sqlx::migrate!`, so
//! new migrations are picked up automatically at compile time.
//!
//! Not every test binary uses every helper, so suppress dead-code warnings
//! for the module as a whole rather than annotating each function.
#![allow(dead_code)]

use sqlx::SqlitePool;
use std::time::Duration;

/// In-memory `auth.db` pool with all auth migrations applied.
pub async fn auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("auth pool");
    sqlx::migrate!("./migrations/auth")
        .run(&pool)
        .await
        .expect("auth migrations");
    pool
}

/// In-memory `chat.db` pool with all chat migrations applied.
pub async fn chat_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("chat pool");
    sqlx::migrate!("./migrations/chat")
        .run(&pool)
        .await
        .expect("chat migrations");
    pool
}

/// In-memory `settings.db` pool with all settings migrations applied.
pub async fn settings_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("settings pool");
    sqlx::migrate!("./migrations/settings")
        .run(&pool)
        .await
        .expect("settings migrations");
    pool
}

/// Open a pool by domain name (`"auth"`, `"chat"`, `"settings"`).
pub async fn pool(name: &str) -> SqlitePool {
    match name {
        "auth" => auth_pool().await,
        "chat" => chat_pool().await,
        "settings" => settings_pool().await,
        other => panic!("unknown db domain: {other}"),
    }
}

/// LC-911: a `tracing::Subscriber` that records every event's fields as a
/// formatted string instead of printing anywhere, so a test can assert on a
/// `tracing::warn!` call (e.g. the embedding-dimension-mismatch log) without
/// pulling in an external log-capture crate. Install it with
/// `tracing::subscriber::set_default`, which stays active on the calling
/// thread (across `.await` points, unlike `with_default`) until the returned
/// guard is dropped - so the test must run on a single-threaded executor
/// (`#[tokio::test(flavor = "current_thread")]`), otherwise the subscriber may
/// not be active on the worker thread that logs the event.
#[derive(Clone, Default)]
pub struct CapturingSubscriber {
    pub events: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

struct FieldsToString(String);
impl tracing::field::Visit for FieldsToString {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        self.0.push_str(&format!("{}={:?}", field.name(), value));
    }
}

impl tracing::Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = FieldsToString(String::new());
        event.record(&mut visitor);
        self.events.lock().unwrap().push(visitor.0);
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

/// Poll `f` every 10ms until it returns `true`, or panic with `what` after a
/// 5s deadline. Use this for any assertion on a side effect produced by a
/// detached `tokio::spawn` task (e.g. push fan-out sends / DB writes): those
/// tasks race the test's own execution, so a fixed sleep before asserting is
/// inherently flaky under load, while polling to a fixed point is not.
pub async fn eventually<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if f().await {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for: {what}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Bounded settle window for asserting the ABSENCE of a spawned-task side
/// effect (e.g. "no push was sent", "token was not deleted"). Polling cannot
/// prove a negative, so this is a fixed wait, not a deadline poll: use it
/// only when there is no positive condition to poll instead, and prefer
/// pairing it with an `eventually` on a control effect in the same test when
/// one exists, so a stalled runtime cannot produce a false-negative pass.
pub async fn settle() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
}
