//! LC-718: the support chat bubble (epic LC-717, Phase 1). A discoverable
//! floating launcher + docked panel that drives the existing support assistant
//! without the user having to know the `/support` and `/human` slash commands.
//!
//! It is a thin front-end over the existing backend: the panel is backed by the
//! viewer's assistant-bot DM room (`help_docs::support_dm_room`), the composer
//! runs `help_docs::handle_support` in that room (placeholder-then-edit answer),
//! and the "Talk to a human" action runs `help_docs::handle_human` (the LC-713
//! escalation + LC-714 ticket fallback + LC-716 claim). No new commands, no new
//! storage. While the panel is open the thread is polled (htmx `every`) so the
//! async bot answer and any admin reply appear; Phase 2 (LC-719) replaces the
//! poll with a WS push and adds the closed-panel attention indicator.

use askama::Template;
use axum::extract::State;
use axum::routing::{get, post};
use axum::Router;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
// Brings the `t` translation filter into scope for the Askama templates below.
use crate::i18n::filters;
use crate::models::User;
use crate::routes::{ai_gate, help_docs};
use crate::state::AppState;
use crate::views::{html, markdown, Html};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/support/bubble", get(get_bubble))
        .route("/support/panel/thread", get(get_thread))
        .route("/support/panel/send", post(post_send))
}

#[derive(Template)]
#[template(path = "support/bubble.html")]
struct BubbleView;

/// One rendered row in the compact panel thread.
struct PanelMsg {
    /// Posted by the viewer (right-aligned bubble) vs. the assistant/admin.
    mine: bool,
    body_html: String,
}

/// LC-719: the conversation stage reflected in the panel, derived from the last
/// assistant reply. Drives the affordance rendered under the thread.
enum Stage {
    /// A normal answer (or the opening state): the composer's actions suffice.
    Normal,
    /// The bot could not answer from the docs: surface a prominent "Still stuck?
    /// Talk to a human" call to action.
    Stuck,
    /// A `/human` fallback filed a ticket: show a "we've filed this (ref #N)" card.
    TicketFiled(i64),
}

#[derive(Template)]
#[template(path = "support/panel_thread.html")]
struct PanelThreadView {
    messages: Vec<PanelMsg>,
    empty: bool,
    stage: Stage,
}

/// True when the support assistant is usable for `user`: the LLM and embeddings
/// are configured, the runtime flag is on, and the viewer is within the AI
/// audience. Mirrors what `/support` itself requires, so the bubble is shown only
/// when clicking it would actually work.
async fn bubble_enabled(state: &AppState, user: &User) -> bool {
    state.llm_available()
        && state.embeddings_available()
        && ai_gate::flag_on(state).await
        && ai_gate::allowed_workspace(state, user).await
}

/// `GET /support/bubble` - the launcher + panel markup, loaded once per page by a
/// tiny `hx-get` slot in the layout. Returns empty when the assistant is not
/// usable for the viewer, so the bubble self-gates without threading a flag
/// through every page's template context.
async fn get_bubble(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    if !bubble_enabled(&state, &user).await {
        return Ok(Html(String::new()));
    }
    html(&BubbleView)
}

/// Build the compact thread view for the viewer's support DM: participant-only
/// bubbles plus the derived [`Stage`] affordance. Shared by the HTTP thread
/// endpoint, the send response, and the WS OOB push, so all three render
/// identically.
async fn build_thread_view(state: &AppState, user: &User) -> Result<PanelThreadView, AppError> {
    let bot = crate::routes::assistant::assistant_bot(state).await?;
    let room = help_docs::support_dm_room(state, user).await?;
    let raw = db::chat::list_messages(&state.chat, room.id).await?;

    // Stage from the last assistant reply: a filed ticket wins over a plain
    // low-confidence decline; a normal answer needs no extra affordance.
    let stage = raw
        .iter()
        .rev()
        .find(|m| m.user_id == bot.id)
        .map(|m| {
            if let Some(id) = help_docs::ticket_ref(&m.body) {
                Stage::TicketFiled(id)
            } else if help_docs::is_low_confidence(&m.body) {
                Stage::Stuck
            } else {
                Stage::Normal
            }
        })
        .unwrap_or(Stage::Normal);

    let messages: Vec<PanelMsg> = raw
        .into_iter()
        .filter(|m| m.user_id == user.id || m.user_id == bot.id)
        .map(|m| PanelMsg {
            mine: m.user_id == user.id,
            body_html: markdown::render(&m.body, &[], &[]),
        })
        .collect();
    let empty = messages.is_empty();
    Ok(PanelThreadView {
        messages,
        empty,
        stage,
    })
}

/// Render the thread partial for an HTTP response (fetch-on-open + send).
async fn render_thread(state: &AppState, user: &User) -> Result<Html, AppError> {
    html(&build_thread_view(state, user).await?)
}

/// LC-719: render the thread as an OOB fragment for a WS push. Wraps the same
/// partial in the `#lc-support-thread` target so htmx's WS extension swaps it in
/// place. Returns `None` on any error (the WS send task then sends nothing).
pub(crate) async fn render_thread_oob(state: &AppState, user: &User) -> Option<String> {
    let view = build_thread_view(state, user).await.ok()?;
    let inner = crate::views::render_template(&view).ok()?;
    Some(format!(
        "<div id=\"lc-support-thread\" hx-swap-oob=\"innerHTML\">{inner}</div>"
    ))
}

/// `GET /support/panel/thread` - the compact thread partial, polled while the
/// panel is open. 403 if the assistant is not usable for the viewer.
async fn get_thread(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    if !bubble_enabled(&state, &user).await {
        return Err(AppError::Forbidden);
    }
    render_thread(&state, &user).await
}

#[derive(serde::Deserialize)]
struct SendForm {
    #[serde(default)]
    body: String,
    /// `"human"` maps to `/human` (escalate); anything else is a `/support`
    /// question.
    #[serde(default)]
    action: String,
}

/// `POST /support/panel/send` - map the panel's friendly input onto the existing
/// backend and return the refreshed thread. A plain message runs `/support`; the
/// "Talk to a human" action runs `/human`. The heavy lifting (RAG, escalation,
/// ticketing) is entirely the existing handlers.
async fn post_send(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<SendForm>,
) -> Result<Html, AppError> {
    if !bubble_enabled(&state, &user).await {
        return Err(AppError::Forbidden);
    }
    let room = help_docs::support_dm_room(&state, &user).await?;
    let body = form.body.trim().to_string();

    if form.action == "human" {
        help_docs::handle_human(&state, &room, &user, &body).await?;
    } else {
        if body.is_empty() {
            return render_thread(&state, &user).await;
        }
        // Echo the user's question into the thread as their own message (the
        // support handler only posts the bot's reply), so the panel reads as a
        // normal chat. Insert directly - no finalize fan-out is needed for the
        // user's own bot DM.
        db::chat::insert_message(&state.chat, room.id, &user.id, &body).await?;
        help_docs::handle_support(&state, &room, &user, &body).await?;
    }
    render_thread(&state, &user).await
}
