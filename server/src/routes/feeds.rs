//! LC-102: per-room read-only RSS/Atom + iCal feeds.
//!
//! The public `GET /feed/rss/{token}` and `GET /feed/ical/{token}` routes are
//! unauthenticated - the token in the URL is the credential - and are merged
//! AFTER the TraceLayer in `build_router` so the token never lands in request
//! logs (mirrors the incoming-webhook route). A feed is bound to its creator;
//! every fetch re-checks that user's current read access to the room, so a
//! kick / room delete / account removal makes the feed return 410. The
//! room-scoped management routes are cookie-authenticated and moderator-gated.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::feeds::{FeedRowView, RoomFeedsPage};
use crate::views::{html, Html};

/// Cap on entries in an RSS/Atom feed (newest first).
const FEED_MESSAGE_LIMIT: i64 = 50;

/// Public router for the unauthenticated feed endpoints. Merged after the
/// TraceLayer in `build_router` so the token is never logged.
pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/feed/rss/{token}", get(get_rss))
        .route("/feed/ical/{token}", get(get_ical))
}

async fn get_rss(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    serve_feed(&state, "rss", &token, &headers).await
}

async fn get_ical(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    serve_feed(&state, "ical", &token, &headers).await
}

/// Shared fetch path for both feed kinds. Returns 404 for an unknown token,
/// 410 for a revoked feed or one whose creator can no longer read the room,
/// 304 when the caller's `If-None-Match` matches, else the rendered feed.
async fn serve_feed(
    state: &AppState,
    kind: &str,
    token: &str,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    // Token is hashed at rest; hashing needs the server secret (same gate as
    // incoming webhooks). No key -> the feature is off -> 404.
    let Some(key) = state.secret_key.as_ref() else {
        return Err(AppError::NotFound);
    };
    let hash = crate::auth::hash_api_token(key, token);
    let Some(feed) = db::room_feeds::find_by_token_hash(&state.chat, &hash).await? else {
        return Err(AppError::NotFound);
    };
    // A token minted for the other kind must not serve via this path.
    if feed.kind != kind {
        return Err(AppError::NotFound);
    }
    if feed.revoked_at.is_some() {
        return Ok(StatusCode::GONE.into_response());
    }
    // Re-check the creator's CURRENT room access. A deleted account, a kick, or
    // a deleted room all turn the feed off (410) without needing to revoke it.
    let Some(creator) = db::auth::find_user_by_id(&state.auth, &feed.created_by).await? else {
        return Ok(StatusCode::GONE.into_response());
    };
    let is_admin = creator.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, feed.room_id, &feed.created_by, is_admin).await? {
        return Ok(StatusCode::GONE.into_response());
    }
    let room = db::chat::get_room(&state.chat, feed.room_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Best-effort last_fetched bump; never block the response.
    let pool = state.chat.clone();
    let id = feed.id;
    tokio::spawn(async move {
        let _ = db::room_feeds::touch_last_fetched(&pool, id).await;
    });

    let (body, content_type, etag) = if kind == "rss" {
        build_atom(state, &room).await?
    } else {
        build_ical(state, &room).await?
    };

    // Conditional GET: a matching If-None-Match (or `*`) means nothing changed.
    if let Some(inm) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        if inm == etag || inm == "*" {
            return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
        }
    }

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, "private, max-age=300".to_string()),
        ],
        body,
    )
        .into_response())
}

// ── feed serialization ──────────────────────────────────────────────────

/// Resolve a message's author display label, caching user lookups across the
/// feed. Webhook / email-ingress / system messages carry an empty user_id.
async fn author_label(
    state: &AppState,
    msg: &db::chat::RawMessage,
    cache: &mut HashMap<String, String>,
) -> String {
    if let Some(wid) = msg.webhook_id {
        if let Ok(Some(id)) = db::webhooks::identity(&state.chat, wid).await {
            return id.name;
        }
        return "Webhook".to_string();
    }
    if msg.email_inbox_id.is_some() {
        return "Email".to_string();
    }
    if msg.is_system {
        return "System".to_string();
    }
    if msg.user_id.is_empty() {
        return "Unknown".to_string();
    }
    if let Some(label) = cache.get(&msg.user_id) {
        return label.clone();
    }
    let label = match db::auth::find_user_by_id(&state.auth, &msg.user_id).await {
        Ok(Some(rec)) => rec
            .display_name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(rec.username),
        _ => "Unknown".to_string(),
    };
    cache.insert(msg.user_id.clone(), label.clone());
    label
}

/// Build the Atom 1.0 document for a room. Returns `(body, content_type, etag)`.
async fn build_atom(
    state: &AppState,
    room: &crate::models::Room,
) -> Result<(String, &'static str, String), AppError> {
    let messages = db::chat::list_recent_messages(&state.chat, room.id, FEED_MESSAGE_LIMIT).await?;

    // ETag fingerprints the rendered set: a new message, an edit, or a deletion
    // all change it, so a conditional GET only 304s when nothing changed.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for m in &messages {
        m.id.hash(&mut hasher);
        m.edited_at.hash(&mut hasher);
    }
    let etag = format!("\"atom-{:x}\"", hasher.finish());

    let feed_updated = messages
        .first()
        .map(|m| rfc3339(effective_ts(m)))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());

    let mut out = String::with_capacity(512 + messages.len() * 256);
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    out.push_str(&format!("  <title>#{}</title>\n", xml_escape(&room.name)));
    out.push_str(&format!("  <id>urn:lets-chat:room:{}</id>\n", room.id));
    out.push_str(&format!("  <updated>{feed_updated}</updated>\n"));
    out.push_str(&format!(
        "  <link rel=\"alternate\" href=\"{}/room/{}\"/>\n",
        xml_escape(&state.base_url),
        room.id
    ));

    let mut cache = HashMap::new();
    for m in &messages {
        let author = author_label(state, m, &mut cache).await;
        let snippet = snippet(&m.body);
        out.push_str("  <entry>\n");
        out.push_str(&format!(
            "    <title>{}: {}</title>\n",
            xml_escape(&author),
            xml_escape(&snippet)
        ));
        out.push_str(&format!("    <id>urn:lets-chat:message:{}</id>\n", m.id));
        out.push_str(&format!(
            "    <published>{}</published>\n",
            rfc3339(&m.created_at)
        ));
        out.push_str(&format!(
            "    <updated>{}</updated>\n",
            rfc3339(effective_ts(m))
        ));
        out.push_str(&format!(
            "    <author><name>{}</name></author>\n",
            xml_escape(&author)
        ));
        out.push_str(&format!(
            "    <link rel=\"alternate\" href=\"{}/room/{}#msg-{}\"/>\n",
            xml_escape(&state.base_url),
            room.id,
            m.id
        ));
        out.push_str(&format!(
            "    <content type=\"text\">{}</content>\n",
            xml_escape(&m.body)
        ));
        out.push_str("  </entry>\n");
    }
    out.push_str("</feed>\n");
    Ok((out, "application/atom+xml; charset=utf-8", etag))
}

/// Build the iCalendar document: one VEVENT per poll (LC-66) that carries a
/// `closes_at`. Returns `(body, content_type, etag)`.
async fn build_ical(
    state: &AppState,
    room: &crate::models::Room,
) -> Result<(String, &'static str, String), AppError> {
    let polls = db::polls::scheduled_for_room(&state.chat, room.id).await?;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (mid, _, closes_at) in &polls {
        mid.hash(&mut hasher);
        closes_at.hash(&mut hasher);
    }
    let etag = format!("\"ical-{:x}\"", hasher.finish());

    let dtstamp = ical_dt(&now_sqlite());
    let mut out = String::with_capacity(256 + polls.len() * 256);
    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//lets-chat//room-feed//EN\r\n");
    out.push_str(&format!("X-WR-CALNAME:#{}\r\n", ical_escape(&room.name)));
    for (mid, question, closes_at) in &polls {
        out.push_str("BEGIN:VEVENT\r\n");
        out.push_str(&format!("UID:lets-chat-poll-{mid}\r\n"));
        out.push_str(&format!("DTSTAMP:{dtstamp}\r\n"));
        out.push_str(&format!("DTSTART:{}\r\n", ical_dt(closes_at)));
        out.push_str(&format!(
            "SUMMARY:Poll closes: {}\r\n",
            ical_escape(question)
        ));
        out.push_str(&format!(
            "DESCRIPTION:{}/room/{}#msg-{}\r\n",
            ical_escape(&state.base_url),
            room.id,
            mid
        ));
        out.push_str("END:VEVENT\r\n");
    }
    out.push_str("END:VCALENDAR\r\n");
    Ok((out, "text/calendar; charset=utf-8", etag))
}

// ── formatting helpers ────────────────────────────────────────────────────

/// The message's effective timestamp: the edit time if edited, else creation.
fn effective_ts(m: &db::chat::RawMessage) -> &str {
    m.edited_at.as_deref().unwrap_or(&m.created_at)
}

/// SQLite `datetime('now')` value, used for DTSTAMP.
fn now_sqlite() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Convert a SQLite `YYYY-MM-DD HH:MM:SS` UTC timestamp to RFC 3339 (Atom).
fn rfc3339(ts: &str) -> String {
    match chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        Ok(dt) => dt.and_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        Err(_) => "1970-01-01T00:00:00Z".to_string(),
    }
}

/// Convert a SQLite UTC timestamp to an iCal UTC datetime (`YYYYMMDDTHHMMSSZ`).
fn ical_dt(ts: &str) -> String {
    match chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        Ok(dt) => dt.and_utc().format("%Y%m%dT%H%M%SZ").to_string(),
        Err(_) => "19700101T000000Z".to_string(),
    }
}

/// First line of a body, trimmed to ~80 chars for a feed title.
fn snippet(body: &str) -> String {
    let line = body.lines().next().unwrap_or("").trim();
    if line.chars().count() > 80 {
        let truncated: String = line.chars().take(79).collect();
        format!("{truncated}\u{2026}")
    } else if line.is_empty() {
        "(no text)".to_string()
    } else {
        line.to_string()
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape iCal TEXT values (RFC 5545 3.3.11) and collapse newlines.
fn ical_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

// ── room-scoped management (cookie-authed, moderator-gated) ──────────────

async fn require_room_manage(
    state: &AppState,
    user: &crate::models::User,
    room_id: i64,
) -> Result<(), AppError> {
    if db::room_rbac::is_room_moderator(&state.chat, room_id, &user.id, &user.role).await? {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn render_page(
    state: &AppState,
    user: &crate::models::User,
    room_id: i64,
    new_url: Option<String>,
    error: Option<String>,
) -> Result<Html, AppError> {
    let room = db::chat::get_room(&state.chat, room_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let current_enclave = super::enclave_for_room(state, room_id).await?;
    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(state, user, current_enclave).await?;
    let feeds: Vec<FeedRowView> = db::room_feeds::list_for_room(&state.chat, room_id)
        .await?
        .into_iter()
        .map(|f| FeedRowView {
            id: f.id,
            kind: f.kind,
            created_at: f.created_at,
            last_fetched_at: f.last_fetched_at,
            revoked: f.revoked,
        })
        .collect();
    html(&RoomFeedsPage {
        user,
        room: &room,
        available: state.secret_key.is_some(),
        feeds: &feeds,
        new_url,
        error,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    })
}

/// GET /room/{id}/feeds
pub async fn get_feeds(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
) -> Result<Html, AppError> {
    require_room_manage(&state, &user, room_id).await?;
    render_page(&state, &user, room_id, None, None).await
}

#[derive(Deserialize)]
pub struct CreateForm {
    /// "rss" or "ical".
    pub kind: String,
}

/// POST /room/{id}/feeds - mint a feed token and reveal its URL once.
pub async fn post_feeds(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    axum::Form(form): axum::Form<CreateForm>,
) -> Result<Response, AppError> {
    require_room_manage(&state, &user, room_id).await?;
    let Some(key) = state.secret_key.as_ref() else {
        return Ok(render_page(
            &state,
            &user,
            room_id,
            None,
            Some("Feeds need a server secret key (LETS_CHAT_SECRET_KEY).".into()),
        )
        .await?
        .into_response());
    };
    let kind = form.kind.trim();
    if kind != "rss" && kind != "ical" {
        return Ok(render_page(
            &state,
            &user,
            room_id,
            None,
            Some("Feed kind must be rss or ical.".into()),
        )
        .await?
        .into_response());
    }
    let token = crate::auth::generate_api_token();
    let hash = crate::auth::hash_api_token(key, &token);
    db::room_feeds::insert(&state.chat, room_id, kind, &hash, &user.id).await?;
    let url = format!("{}/feed/{}/{}", state.base_url, kind, token);
    Ok(render_page(&state, &user, room_id, Some(url), None)
        .await?
        .into_response())
}

/// POST /room/{id}/feeds/{fid}/revoke
pub async fn post_revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((room_id, fid)): Path<(i64, i64)>,
) -> Result<Redirect, AppError> {
    require_room_manage(&state, &user, room_id).await?;
    db::room_feeds::revoke(&state.chat, fid, room_id).await?;
    Ok(Redirect::to(&format!("/room/{room_id}/feeds")))
}
