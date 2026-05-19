//! Link-filter rules and quarantine queue (LC-94).

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterAction {
    Block,
    Quarantine,
    Warn,
}

impl FilterAction {
    pub fn as_str(self) -> &'static str {
        match self {
            FilterAction::Block => "block",
            FilterAction::Quarantine => "quarantine",
            FilterAction::Warn => "warn",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "block" => Some(FilterAction::Block),
            "quarantine" => Some(FilterAction::Quarantine),
            "warn" => Some(FilterAction::Warn),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LinkFilterRule {
    pub id: i64,
    pub pattern: String,
    pub action: FilterAction,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct QuarantineEntry {
    pub message_id: i64,
    pub room_id: i64,
    pub author_id: String,
    pub body: String,
    pub matched_pattern: String,
    pub matched_url: String,
    pub created_at: String,
    pub reviewed_at: Option<String>,
    pub reviewed_by: Option<String>,
    pub decision: Option<String>,
}

pub async fn list_rules(pool: &SqlitePool) -> Result<Vec<LinkFilterRule>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, pattern, action, created_by, created_at \
         FROM link_filter_rules ORDER BY pattern ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| LinkFilterRule {
            id: r.get("id"),
            pattern: r.get("pattern"),
            action: FilterAction::parse(r.get::<&str, _>("action")).unwrap_or(FilterAction::Warn),
            created_by: r.get("created_by"),
            created_at: r.get("created_at"),
        })
        .collect())
}

pub async fn insert_rule(
    pool: &SqlitePool,
    pattern: &str,
    action: FilterAction,
    created_by: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO link_filter_rules (pattern, action, created_by) \
         VALUES (?, ?, ?)",
    )
    .bind(pattern)
    .bind(action.as_str())
    .bind(created_by)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

pub async fn delete_rule(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM link_filter_rules WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// First rule that matches any host in `hosts`. Rules are scanned in
/// pattern-alphabetical order (same as `list_rules`) so behavior is
/// stable across runs; the first hit wins regardless of action
/// severity. Returns the rule + the matched host so the caller can
/// log which URL tripped which rule.
pub async fn find_match(
    pool: &SqlitePool,
    hosts: &[String],
) -> Result<Option<(LinkFilterRule, String)>, sqlx::Error> {
    if hosts.is_empty() {
        return Ok(None);
    }
    let rules = list_rules(pool).await?;
    for host in hosts {
        for rule in &rules {
            if crate::links::pattern_matches(&rule.pattern, host) {
                return Ok(Some((rule.clone(), host.clone())));
            }
        }
    }
    Ok(None)
}

/// Insert the quarantine record for a freshly-inserted message. The
/// caller is responsible for setting `messages.quarantined = 1` in the
/// same transaction (or, in our non-transactional path, immediately
/// after `insert_message`).
pub async fn insert_quarantine(
    pool: &SqlitePool,
    message_id: i64,
    matched_pattern: &str,
    matched_url: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO link_filter_quarantine (message_id, matched_pattern, matched_url) \
         VALUES (?, ?, ?)",
    )
    .bind(message_id)
    .bind(matched_pattern)
    .bind(matched_url)
    .execute(pool)
    .await?;
    Ok(())
}

/// List quarantined messages awaiting review, newest first. Joins
/// against `messages` so the admin queue can render the body + author
/// in one query.
pub async fn list_pending_quarantine(
    pool: &SqlitePool,
) -> Result<Vec<QuarantineEntry>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT q.message_id, m.room_id, m.user_id AS author_id, m.body, \
                q.matched_pattern, q.matched_url, q.created_at, \
                q.reviewed_at, q.reviewed_by, q.decision \
         FROM link_filter_quarantine q \
         JOIN messages m ON m.id = q.message_id \
         WHERE q.reviewed_at IS NULL \
         ORDER BY q.created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| QuarantineEntry {
            message_id: r.get("message_id"),
            room_id: r.get("room_id"),
            author_id: r.get("author_id"),
            body: r.get("body"),
            matched_pattern: r.get("matched_pattern"),
            matched_url: r.get("matched_url"),
            created_at: r.get("created_at"),
            reviewed_at: r.get("reviewed_at"),
            reviewed_by: r.get("reviewed_by"),
            decision: r.get("decision"),
        })
        .collect())
}

/// Approve a quarantined message: clear the flag on `messages` so it
/// appears in the room and record the admin's decision.
pub async fn approve_quarantine(
    pool: &SqlitePool,
    message_id: i64,
    reviewer: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE messages SET quarantined = 0 WHERE id = ?")
        .bind(message_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE link_filter_quarantine \
         SET reviewed_by = ?, reviewed_at = datetime('now'), decision = 'approve' \
         WHERE message_id = ?",
    )
    .bind(reviewer)
    .bind(message_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Reject a quarantined message: soft-delete the message and record
/// the admin's decision. The quarantine row stays for the audit
/// trail; the message body is no longer reachable to anyone.
pub async fn reject_quarantine(
    pool: &SqlitePool,
    message_id: i64,
    reviewer: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE messages SET deleted_at = datetime('now'), deleted_by = ? \
         WHERE id = ?",
    )
    .bind(reviewer)
    .bind(message_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE link_filter_quarantine \
         SET reviewed_by = ?, reviewed_at = datetime('now'), decision = 'reject' \
         WHERE message_id = ?",
    )
    .bind(reviewer)
    .bind(message_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Count of unreviewed entries, for the admin sidebar badge / nav.
pub async fn count_pending_quarantine(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM link_filter_quarantine WHERE reviewed_at IS NULL")
        .fetch_one(pool)
        .await
}
