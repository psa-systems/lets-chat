//! Single-writer background worker for high-frequency, low-value DB writes.
//!
//! `users.last_active_at` and `sessions.last_seen_at` are updated on every
//! authed request, but the columns only matter at minute resolution. Doing
//! the writes inline blocks the response; doing them via per-request
//! `tokio::spawn` saturates the SQLite pool under load and stalls every
//! other handler waiting for a connection.
//!
//! The worker owns one mpsc receiver and drains it on a 500 ms tick,
//! deduplicating by id and flushing the resulting set in a single
//! `UPDATE ... WHERE id IN (...)`. Senders only push an id onto the
//! channel, which is a lock-free operation; they never touch the pool.

use std::collections::HashSet;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::mpsc;

/// Channel size. Generously oversized: under steady load the worker
/// drains it every 500 ms, so the channel is normally near-empty. A
/// bounded channel still matters: if the worker stalls, senders that
/// would have piled up via `tokio::spawn` instead see a full channel and
/// drop the touch, which is the right tradeoff for these columns.
const CHANNEL_CAPACITY: usize = 4096;

/// How often the worker flushes accumulated touches.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum Touch {
    User(String),
    Session(String),
}

#[derive(Clone)]
pub struct BgWriter {
    tx: mpsc::Sender<Touch>,
}

impl BgWriter {
    /// Send a user-activity touch. Silently drops the request if the
    /// background worker has stalled (full channel) - the cost of
    /// missing a `last_active_at` bump is far smaller than blocking the
    /// HTTP path on it.
    pub fn touch_user(&self, user_id: &str) {
        let _ = self.tx.try_send(Touch::User(user_id.to_string()));
    }

    /// Send a session last-seen touch. Same drop-on-overflow semantics
    /// as `touch_user`.
    pub fn touch_session(&self, session_id: &str) {
        let _ = self.tx.try_send(Touch::Session(session_id.to_string()));
    }
}

/// Spawn the background writer and return the sender handle for use in
/// `AppState`. The worker exits when the sender side is dropped (i.e.
/// the whole process is shutting down).
pub fn spawn(auth: SqlitePool) -> BgWriter {
    let (tx, mut rx) = mpsc::channel::<Touch>(CHANNEL_CAPACITY);

    tokio::spawn(async move {
        let mut tick = tokio::time::interval(FLUSH_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut users: HashSet<String> = HashSet::new();
        let mut sessions: HashSet<String> = HashSet::new();

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if !users.is_empty() {
                        let batch: Vec<String> = users.drain().collect();
                        if let Err(e) = flush_user_activity(&auth, &batch).await {
                            tracing::warn!(error = %e, count = batch.len(), "flush user activity failed");
                        }
                    }
                    if !sessions.is_empty() {
                        let batch: Vec<String> = sessions.drain().collect();
                        if let Err(e) = flush_session_last_seen(&auth, &batch).await {
                            tracing::warn!(error = %e, count = batch.len(), "flush session last-seen failed");
                        }
                    }
                }
                maybe = rx.recv() => {
                    match maybe {
                        Some(Touch::User(id)) => { users.insert(id); }
                        Some(Touch::Session(id)) => { sessions.insert(id); }
                        None => break,
                    }
                }
            }
        }
    });

    BgWriter { tx }
}

/// Batch-update `users.last_active_at` for every id in `ids`. The
/// idle->active flip is intentionally not handled here: an idle->active
/// flip needs to broadcast a status-change event, so it stays on the
/// hot path via `db::auth::touch_user_activity`. The batch path is only
/// for the routine "stay active" touches that don't change status.
async fn flush_user_activity(pool: &SqlitePool, ids: &[String]) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE users SET last_active_at = datetime('now') \
         WHERE id IN ({placeholders}) AND status != 'idle'"
    );
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    q.execute(pool).await?;
    Ok(())
}

/// Batch-update `sessions.last_seen_at` for every id in `ids`.
async fn flush_session_last_seen(pool: &SqlitePool, ids: &[String]) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql =
        format!("UPDATE sessions SET last_seen_at = datetime('now') WHERE id IN ({placeholders})");
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    q.execute(pool).await?;
    Ok(())
}
