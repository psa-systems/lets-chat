//! LC-549: embeddings-backed "Find related messages".
//!
//! A per-message action ranks other messages in the same room by cosine
//! similarity of their stored embeddings and shows the nearest few in the shared
//! right-side `#thread-panel` slot. Gated on an operator embeddings endpoint
//! (`crate::embeddings`); the menu item is CSS-hidden when none is configured.
//!
//! Also hosts `embed_message`, the best-effort background populator called from
//! `post_message`, so a message becomes searchable shortly after it is sent.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};

use crate::auth::AuthUser;
use crate::db;
use crate::db::chat::RawMessage;
use crate::embeddings;
use crate::error::AppError;
#[allow(unused_imports)]
use crate::i18n::filters;
use crate::state::AppState;
use crate::views::{html, Html};
use askama::Template; // LC-188: in-scope for the |t template filter.

/// How many related messages to show.
const TOP_K: usize = 5;
/// Minimum cosine similarity to count as "related" (drop near-orthogonal noise
/// so an empty result reads honestly rather than padding with unrelated hits).
const MIN_SIMILARITY: f32 = 0.15;
/// `display_name` if non-empty, else `username`.
fn label_for(username: &str, display_name: Option<&str>) -> String {
    match display_name {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => username.to_string(),
    }
}

/// Embed `text` and upsert it as `message_id`'s vector. Best-effort: an
/// embeddings-endpoint failure is logged and swallowed (the message simply will
/// not surface in semantic results), so it never fails a send. No-op when no
/// embeddings client is configured. Reused by `post_message`'s background task.
pub(crate) async fn embed_message(
    state: &AppState,
    message_id: i64,
    room_id: i64,
    text: &str,
) -> Result<(), sqlx::Error> {
    let Some(client) = state.embedding_client.clone() else {
        return Ok(());
    };
    // LC-679: the runtime kill switch also halts background message embedding, so
    // the whole embeddings surface goes dark when the flag is off (flag-only; no
    // user context on this populator path).
    if !super::ai_gate::flag_on(state).await {
        return Ok(());
    }
    let vec = match client.embed(text).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, message_id, "embedding request failed");
            return Ok(());
        }
    };
    let bytes = embeddings::vec_to_bytes(&vec);
    db::message_embeddings::upsert(&state.chat, message_id, room_id, vec.len() as i64, &bytes).await
}

/// LC-673: embed a batch of messages that have no embedding yet - history from
/// before the embeddings endpoint was configured, which is otherwise invisible
/// to semantic search. Best-effort per message ([`embed_message`] swallows
/// endpoint errors), so a transient failure just leaves that message for a later
/// tick. Returns how many messages were processed this batch; `0` means the
/// backlog is drained (or no embeddings client is configured), which the caller
/// uses to idle. Newest-first, since recent history is the likeliest search
/// target.
pub async fn run_embedding_backfill_tick(
    state: &AppState,
    batch: i64,
) -> Result<usize, sqlx::Error> {
    if state.embedding_client.is_none() {
        return Ok(0);
    }
    let pending = db::message_embeddings::list_unembedded(&state.chat, batch).await?;
    let n = pending.len();
    for (id, room_id, body) in pending {
        embed_message(state, id, room_id, &body).await?;
    }
    Ok(n)
}

/// One related-message row.
struct RelatedItem {
    room_id: i64,
    message_id: i64,
    author: String,
    snippet: String,
    created_at: String,
}

#[derive(Template)]
#[template(path = "room/related_panel.html")]
struct RelatedPanel {
    items: Vec<RelatedItem>,
}

/// GET /messages/{message_id}/related - rank other messages in the same room by
/// embedding similarity to this one and render the nearest few.
pub async fn get_related(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
) -> Result<Html, AppError> {
    let Some(client) = state.embedding_client.clone() else {
        return Err(AppError::BadRequest(
            "related search is not configured".into(),
        ));
    };

    let source = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, source.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    // LC-679: runtime flag + role gate for the embeddings surface (403 for
    // an unprivileged/flag-off caller).
    super::ai_gate::require_embeddings_in_room(&state, source.room_id, &user).await?;

    // Source vector: prefer the stored one; otherwise embed on demand (the
    // background populator may not have reached this message yet).
    let query_vec = match db::message_embeddings::get(&state.chat, message_id).await? {
        Some(v) => v,
        None => {
            if source.body.trim().is_empty() {
                return Err(AppError::BadRequest("nothing to compare".into()));
            }
            match client.embed(&source.body).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "related-search embed failed");
                    return Err(AppError::BadRequest("related search failed".into()));
                }
            }
        }
    };

    // Cosine-rank every other embedding in the room, keep the top-K above the
    // relevance floor.
    let candidates =
        db::message_embeddings::list_for_room(&state.chat, source.room_id, Some(message_id))
            .await?;
    let mut scored: Vec<(i64, f32)> = candidates
        .into_iter()
        .map(|c| {
            (
                c.message_id,
                embeddings::cosine_similarity(&query_vec, &c.vec),
            )
        })
        .filter(|(_, s)| *s >= MIN_SIMILARITY)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(TOP_K);

    // Load the ranked messages (skipping any deleted since embedding), preserving
    // rank order, and resolve author labels in one bulk query.
    let mut msgs: Vec<RawMessage> = Vec::with_capacity(scored.len());
    for (id, _) in &scored {
        if let Some(m) = db::chat::get_message(&state.chat, *id).await? {
            msgs.push(m);
        }
    }
    let mut unique: HashSet<&str> = HashSet::new();
    for m in &msgs {
        unique.insert(m.user_id.as_str());
    }
    let ids: Vec<&str> = unique.into_iter().collect();
    let resolved = db::auth::display_names_for_ids(&state.auth, &ids).await?;
    let labels: HashMap<String, String> = resolved
        .into_iter()
        .map(|(id, (u, d))| (id, label_for(&u, d.as_deref())))
        .collect();

    let items: Vec<RelatedItem> = msgs
        .into_iter()
        .map(|m| RelatedItem {
            room_id: m.room_id,
            message_id: m.id,
            author: labels
                .get(&m.user_id)
                .cloned()
                .unwrap_or_else(|| m.user_id.clone()),
            snippet: m.body,
            created_at: m.created_at,
        })
        .collect();

    html(&RelatedPanel { items })
}
