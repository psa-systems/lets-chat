//! LC-718: the support chat bubble (epic LC-717, Phase 1). A discoverable
//! floating launcher + docked panel that drives the existing support assistant
//! without the user having to know the `/support` and `/human` slash commands.
//!
//! It is a thin front-end over the existing backend: the panel is backed by the
//! viewer's assistant-bot DM room (`help_docs::support_dm_room`), the composer
//! runs `help_docs::handle_support` in that room (placeholder-then-edit answer),
//! and the "Talk to a human" action runs `help_docs::handle_human` (the LC-713
//! escalation + LC-714 ticket fallback + LC-716 claim). No new commands, no new
//! storage. The thread fetches once on open and stays live via the
//! `SupportThreadChanged` WS push (LC-719); that phase also adds the stage-aware
//! footer and the closed-panel attention dot. LC-720 lifts cited Sources into
//! link chips and pushes the panel on claim/resolve notifications.

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
        // LC-795: one older page of bubbles for the panel's load-older sentinel.
        .route("/support/panel/thread/older", get(get_older_thread))
        .route("/support/panel/send", post(post_send))
        .route("/support/panel/ticket", post(post_ticket_details))
}

/// LC-795: bubbles the panel renders per page. Matches the thread panel's
/// `THREAD_REPLY_PAGE_LIMIT`: the panel is short, and older pages arrive through
/// the top-of-list sentinel, so nothing is out of reach.
const PANEL_PAGE_LIMIT: i64 = 50;

/// LC-724: substring that marks the "you added details" confirmation, so the
/// panel derives the ticket-filed stage from it (kept next to the message it
/// matches, in [`post_ticket_details`]).
const DETAILS_FILED_MARKER: &str = "added your details";

/// LC-724: true when a bot message is a `/human` waiting confirmation (an admin
/// was notified, or none were available). Matches the stable marker phrases in
/// [`help_docs::handle_human`]; both variants also carry the ticket id.
fn is_waiting_confirmation(body: &str) -> bool {
    body.contains("an admin has been notified") || body.contains("no admins are available")
}

/// LC-724: turn a SQLite `datetime('now')` UTC timestamp ("YYYY-MM-DD HH:MM:SS")
/// into an ISO-8601 UTC string ("YYYY-MM-DDTHH:MM:SSZ") that browsers parse as
/// UTC, so the panel's client-side "waiting" elapsed timer starts from the right
/// instant regardless of the viewer's timezone.
fn to_iso_utc(sqlite_ts: &str) -> String {
    let t = sqlite_ts.trim();
    if t.is_empty() {
        return String::new();
    }
    format!("{}Z", t.replace(' ', "T"))
}

/// LC-724: derive the panel stage from the last assistant message and its time.
/// Order matters: an "added details" confirmation is the filed stage; a `/human`
/// waiting confirmation (which carries the ticket id) is the waiting stage; a
/// low-confidence decline is the stuck stage; anything else needs no affordance.
fn derive_stage(body: &str, created_at: &str) -> Stage {
    if body.contains(DETAILS_FILED_MARKER) {
        if let Some(id) = help_docs::ticket_ref(body) {
            return Stage::TicketFiled(id);
        }
    }
    if is_waiting_confirmation(body) {
        if let Some(id) = help_docs::ticket_ref(body) {
            return Stage::Waiting(id, to_iso_utc(created_at));
        }
    }
    if help_docs::is_low_confidence(body) {
        return Stage::Stuck;
    }
    Stage::Normal
}

#[derive(Template)]
#[template(path = "support/bubble.html")]
struct BubbleView;

/// One cited documentation source, rendered as a link chip under a bot reply.
struct SourceChip {
    label: String,
    url: String,
}

/// One rendered row in the compact panel thread.
struct PanelMsg {
    /// Posted by the viewer (right-aligned bubble) vs. the assistant/admin.
    mine: bool,
    body_html: String,
    /// LC-720: cited sources lifted out of the reply body and rendered as chips.
    sources: Vec<SourceChip>,
    /// LC-730: this is the transient "checking the docs" placeholder (see
    /// [`help_docs::is_thinking_placeholder`]); the template renders the animated
    /// skeleton loader instead of `body_html`, and it is replaced in place by the
    /// real answer via the `SupportThreadChanged` push.
    pending: bool,
    /// LC-730: ISO-UTC time the placeholder was posted, so the client can rotate
    /// the staged "searching -> reading -> writing" wording by elapsed time. Empty
    /// unless `pending`.
    pending_since: String,
    /// LC-732: show the assistant avatar beside this bubble. True only for the
    /// first message of a consecutive assistant run (and never for the viewer's own
    /// messages), so a grouped set of bot replies shows one avatar, not one each.
    show_avatar: bool,
}

/// LC-720: split a bot reply into its prose and its cited `Sources`, so the panel
/// can render the citations as first-class link chips instead of a trailing
/// markdown bullet list. Matches the `\n\n**Sources:**\n- {product}: {title}
/// ({url})` block appended by [`help_docs::build_support_answer`]. A body without
/// the marker (user messages, declines) returns the whole body and no chips.
fn split_sources(body: &str) -> (&str, Vec<SourceChip>) {
    const MARKER: &str = "\n\n**Sources:**";
    let Some(idx) = body.find(MARKER) else {
        return (body, Vec::new());
    };
    let chips = body[idx + MARKER.len()..]
        .lines()
        .filter_map(parse_source_line)
        .collect();
    (&body[..idx], chips)
}

/// LC-723: drop a leading `> ...` blockquote paragraph. Support answers are
/// stored with a `> {asker}: {question}` attribution header (see
/// [`help_docs::build_support_answer`]); in the panel the asker's own question is
/// already shown as their own bubble, so echoing it back in the bot reply is
/// redundant. Removes only the first paragraph, and only when it is a blockquote.
fn strip_leading_quote(body: &str) -> &str {
    match body.strip_prefix("> ") {
        Some(rest) => match rest.find("\n\n") {
            Some(idx) => &rest[idx + 2..],
            None => body,
        },
        None => body,
    }
}

/// Parse one `- {label} ({url})` source line into a chip. The label keeps the
/// `product: title` text; the URL is the last parenthesised http(s) link.
fn parse_source_line(line: &str) -> Option<SourceChip> {
    let line = line.trim().strip_prefix("- ")?;
    let open = line.rfind(" (")?;
    let url = line[open + 2..].strip_suffix(')')?;
    if !url.starts_with("http") {
        return None;
    }
    Some(SourceChip {
        label: line[..open].to_string(),
        url: url.to_string(),
    })
}

/// LC-719: the conversation stage reflected in the panel, derived from the last
/// assistant reply. Drives the affordance rendered under the thread.
enum Stage {
    /// A normal answer (or the opening state): the composer's actions suffice.
    Normal,
    /// The bot could not answer from the docs: surface a prominent "Still stuck?
    /// Talk to a human" call to action.
    Stuck,
    /// LC-724: the user asked for a human and a ticket was opened (ref #N), but no
    /// details have been added yet: show the live "waiting for a human" state (an
    /// elapsed timer keyed on the ISO-UTC start time, a timeout re-prompt, and an
    /// "add details" form). The `String` is the ISO-UTC time the wait began.
    Waiting(i64, String),
    /// A `/human` fallback filed a ticket: show a "we've filed this (ref #N)" card.
    TicketFiled(i64),
}

#[derive(Template)]
#[template(path = "support/panel_thread.html")]
struct PanelThreadView {
    messages: Vec<PanelMsg>,
    empty: bool,
    stage: Stage,
    /// LC-795: the endpoint that returns the page before this one, or `None`
    /// when this page already reaches the start of the conversation. `Some`
    /// renders the top-of-list load-older sentinel.
    older_page_url: Option<String>,
}

/// LC-795: one older page of bubbles, swapped in over the sentinel that fetched
/// it. Carries no stage: the stage is derived from the LAST assistant reply,
/// which is always on the newest page.
#[derive(Template)]
#[template(path = "support/panel_older.html")]
struct PanelOlderView {
    messages: Vec<PanelMsg>,
    older_page_url: Option<String>,
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
///
/// LC-795: reads ONE page of [`PANEL_PAGE_LIMIT`] messages. `before_id` is the
/// load-older cursor; `None` is the newest page, which is what a fresh panel
/// open renders. `older_page_url` on the result is `Some` exactly when the read
/// saw a row behind the page, and is what renders the sentinel.
async fn build_thread_view(
    state: &AppState,
    user: &User,
    before_id: Option<i64>,
) -> Result<PanelThreadView, AppError> {
    let bot = crate::routes::assistant::assistant_bot(state).await?;
    let room = help_docs::support_dm_room(state, user).await?;
    // Read `limit + 1` rows: the overflow row is the "older messages exist"
    // answer, without a second COUNT query.
    let mut raw =
        db::chat::list_messages_paginated(&state.chat, room.id, before_id, PANEL_PAGE_LIMIT + 1)
            .await?;
    let has_older = raw.len() as i64 > PANEL_PAGE_LIMIT;
    raw.truncate(PANEL_PAGE_LIMIT as usize);
    // `list_messages_paginated` returns newest-first; the panel renders oldest
    // -> newest, like every other timeline.
    raw.reverse();
    // Taken BEFORE the participant filter, so the next cursor steps past a
    // filtered-out row instead of re-reading it forever.
    let older_page_url = has_older
        .then(|| raw.first().map(|m| m.id))
        .flatten()
        .map(|cursor| format!("/support/panel/thread/older?before={cursor}"));

    // Stage from the last assistant reply and its timestamp (the waiting timer
    // needs the start instant). See [`derive_stage`] for the precedence.
    let stage = raw
        .iter()
        .rev()
        .find(|m| m.user_id == bot.id)
        .map(|m| derive_stage(&m.body, &m.created_at))
        .unwrap_or(Stage::Normal);

    // LC-732: `prev_bot` tracks whether the previously pushed bubble was the
    // assistant, so the avatar shows once per consecutive assistant run.
    let mut messages: Vec<PanelMsg> = Vec::new();
    let mut prev_bot = false;
    for m in raw
        .into_iter()
        .filter(|m| m.user_id == user.id || m.user_id == bot.id)
    {
        let mine = m.user_id == user.id;
        let (main, sources) = split_sources(&m.body);
        // LC-723: strip the redundant attribution header from bot replies (the
        // asker's question already shows as their own bubble in the panel).
        let main = if mine {
            main
        } else {
            strip_leading_quote(main)
        };
        // LC-730: the "checking the docs" placeholder renders as an animated
        // skeleton rather than flat italic text; skip the markdown render and
        // carry its start time so the client can stage the wording.
        let pending = !mine && help_docs::is_thinking_placeholder(main);
        let show_avatar = !mine && !prev_bot;
        prev_bot = !mine;
        messages.push(PanelMsg {
            mine,
            body_html: if pending {
                String::new()
            } else {
                markdown::render(main, &[], &[])
            },
            sources,
            pending,
            pending_since: if pending {
                to_iso_utc(&m.created_at)
            } else {
                String::new()
            },
            show_avatar,
        });
    }
    // LC-795: only a conversation with nothing in it anywhere gets the welcome
    // block. A page whose rows all filtered out still has history behind it.
    let empty = messages.is_empty() && older_page_url.is_none();
    Ok(PanelThreadView {
        messages,
        empty,
        stage,
        older_page_url,
    })
}

/// Render the thread partial for an HTTP response (fetch-on-open + send).
async fn render_thread(state: &AppState, user: &User) -> Result<Html, AppError> {
    html(&build_thread_view(state, user, None).await?)
}

/// LC-719: render the thread as an OOB fragment for a WS push. Wraps the same
/// partial in the `#lc-support-thread` target so htmx's WS extension swaps it in
/// place. Returns `None` on any error - the WS send task then sends nothing, so
/// the failure is logged here at `warn` with its cause; a push that silently
/// vanished would otherwise look exactly like "no update to send".
pub(crate) async fn render_thread_oob(state: &AppState, user: &User) -> Option<String> {
    let view = build_thread_view(state, user, None)
        .await
        .inspect_err(
            |e| tracing::warn!(user = %user.id, error = %e, "support thread push: build failed"),
        )
        .ok()?;
    let inner = crate::views::render_template(&view)
        .inspect_err(
            |e| tracing::warn!(user = %user.id, error = %e, "support thread push: render failed"),
        )
        .ok()?;
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
struct OlderQuery {
    before: Option<i64>,
}

/// `GET /support/panel/thread/older?before={id}` - LC-795: the page of bubbles
/// OLDER than `before`, preceded by the next sentinel while history remains.
/// The panel's top-of-list sentinel swaps this in over itself, so each fetched
/// page carries the cursor for the one behind it until the first turn is on
/// screen. Gated exactly like the panel itself.
async fn get_older_thread(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::extract::Query(q): axum::extract::Query<OlderQuery>,
) -> Result<Html, AppError> {
    if !bubble_enabled(&state, &user).await {
        return Err(AppError::Forbidden);
    }
    let view = build_thread_view(&state, &user, q.before).await?;
    html(&PanelOlderView {
        messages: view.messages,
        older_page_url: view.older_page_url,
    })
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

/// LC-724: the in-panel "add details" form. Enriches the ticket opened when the
/// user asked for a human; every field is optional.
#[derive(serde::Deserialize)]
struct DetailForm {
    ticket_id: i64,
    #[serde(default)]
    need: String,
    #[serde(default)]
    tried: String,
    #[serde(default)]
    urgency: String,
    #[serde(default)]
    email: String,
}

/// Assemble the enriched ticket body from the detail form. Only non-empty fields
/// are included, so a sparse submission still reads cleanly in the admin queue,
/// and an entirely empty one keeps a sensible placeholder.
fn compose_ticket_body(form: &DetailForm) -> String {
    let mut parts: Vec<String> = Vec::new();
    let need = form.need.trim();
    if !need.is_empty() {
        parts.push(need.to_string());
    }
    let tried = form.tried.trim();
    if !tried.is_empty() {
        parts.push(format!("What they tried: {tried}"));
    }
    let urgency = form.urgency.trim();
    if !urgency.is_empty() {
        parts.push(format!("Urgency: {urgency}"));
    }
    let email = form.email.trim();
    if !email.is_empty() {
        parts.push(format!("Contact: {email}"));
    }
    if parts.is_empty() {
        return "(the user asked for a human via the support panel)".to_string();
    }
    parts.join("\n\n")
}

/// `POST /support/panel/ticket` - LC-724: the requester enriches their still-open
/// support ticket with the structured detail from the in-panel form. The update
/// is scoped in the DB to the caller and to the open state, so it can only touch
/// the user's own not-yet-handled ticket. On success it refreshes the admin queue
/// and posts a bot confirmation, moving the panel to the "ticket filed" stage; if
/// the ticket is gone or already claimed/resolved it just re-renders the thread.
async fn post_ticket_details(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<DetailForm>,
) -> Result<Html, AppError> {
    if !bubble_enabled(&state, &user).await {
        return Err(AppError::Forbidden);
    }
    let room = help_docs::support_dm_room(&state, &user).await?;
    let body = compose_ticket_body(&form);
    let updated =
        db::support_tickets::update_body(&state.chat, form.ticket_id, &user.id, &body).await?;
    if !updated {
        return render_thread(&state, &user).await;
    }
    crate::routes::support::broadcast_support_changed(&state);

    let bot = crate::routes::assistant::assistant_bot(&state).await?;
    let confirmation = format!(
        "_Thanks - I've added your details to support ticket #{}. An admin will follow up._",
        form.ticket_id
    );
    let msg_id = db::chat::insert_message(&state.chat, room.id, &bot.id, &confirmation).await?;
    crate::routes::room::finalize_message_send(&state, &room, &bot, msg_id, &confirmation, None)
        .await?;
    render_thread(&state, &user).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sources_extracts_chips_and_strips_the_block() {
        let body = "To reset, open Settings.\n\n**Sources:**\n\
            - mokosh-server: Configuration (https://a8n.systems/apps/mokosh-server/docs/configuration)\n\
            - mokosh-www: Theming (https://a8n.systems/apps/mokosh-www/docs/theming)";
        let (main, chips) = split_sources(body);
        assert_eq!(main, "To reset, open Settings.");
        assert_eq!(chips.len(), 2);
        assert_eq!(chips[0].label, "mokosh-server: Configuration");
        assert_eq!(
            chips[0].url,
            "https://a8n.systems/apps/mokosh-server/docs/configuration"
        );
        assert_eq!(chips[1].label, "mokosh-www: Theming");
    }

    #[test]
    fn split_sources_without_marker_returns_body_unchanged() {
        let (main, chips) = split_sources("just a plain answer with no citations");
        assert_eq!(main, "just a plain answer with no citations");
        assert!(chips.is_empty());
    }

    #[test]
    fn thinking_placeholder_renders_the_animated_skeleton() {
        // The "checking the docs" placeholder is detected (post header-strip)...
        assert!(help_docs::is_thinking_placeholder(
            "_The support assistant is checking the docs\u{2026}_"
        ));
        assert!(!help_docs::is_thinking_placeholder("Here is your answer."));

        // ...and the panel renders the shared AI skeleton loader (with the start
        // time for the staged wording) in place of the flat italic line.
        let view = PanelThreadView {
            messages: vec![PanelMsg {
                mine: false,
                body_html: String::new(),
                sources: Vec::new(),
                pending: true,
                pending_since: "2026-08-16T00:46:02Z".into(),
                show_avatar: true,
            }],
            empty: false,
            stage: Stage::Normal,
            older_page_url: None,
        };
        let html = view.render().unwrap();
        assert!(
            html.contains("data-lc-support-thinking") && html.contains("lc-ai-skel"),
            "pending message renders the skeleton loader, got: {html}"
        );
        assert!(
            html.contains("data-since=\"2026-08-16T00:46:02Z\""),
            "the skeleton carries the start time for staged wording, got: {html}"
        );
        assert!(
            !html.contains("checking the docs"),
            "the flat italic placeholder text is replaced, got: {html}"
        );
    }

    #[test]
    fn to_iso_utc_makes_a_browser_parseable_utc_stamp() {
        assert_eq!(to_iso_utc("2026-08-16 00:46:02"), "2026-08-16T00:46:02Z");
        assert_eq!(to_iso_utc("  2026-08-16 00:46:02 "), "2026-08-16T00:46:02Z");
        assert_eq!(to_iso_utc(""), "");
    }

    #[test]
    fn derive_stage_maps_each_confirmation_to_its_affordance() {
        // The two /human confirmations -> the waiting stage, keyed on the ticket.
        let waiting_available = "_bob, an admin has been notified and usually replies within about 5 minutes. I've opened support ticket #12 so this doesn't get lost - you can keep waiting, or add details below._";
        assert!(matches!(
            derive_stage(waiting_available, "2026-08-16 00:46:02"),
            Stage::Waiting(12, ref s) if s == "2026-08-16T00:46:02Z"
        ));
        let waiting_unavailable = "_bob, no admins are available right now, so I've filed support ticket #13 for follow-up. You can add details below._";
        assert!(matches!(
            derive_stage(waiting_unavailable, "2026-08-16 00:46:02"),
            Stage::Waiting(13, _)
        ));
        // The "added details" confirmation -> the filed stage.
        let filed =
            "_Thanks - I've added your details to support ticket #12. An admin will follow up._";
        assert!(matches!(derive_stage(filed, ""), Stage::TicketFiled(12)));
        // A low-confidence decline -> stuck; a normal answer -> normal.
        assert!(matches!(
            derive_stage(
                "_I couldn't find anything about that in the product documentation._",
                ""
            ),
            Stage::Stuck
        ));
        assert!(matches!(
            derive_stage("Here is how to configure it.", ""),
            Stage::Normal
        ));
    }

    #[test]
    fn compose_ticket_body_joins_only_the_filled_fields() {
        let form = DetailForm {
            ticket_id: 1,
            need: "  Reset my password  ".into(),
            tried: "the reset link".into(),
            urgency: "high".into(),
            email: "".into(),
        };
        let body = compose_ticket_body(&form);
        assert_eq!(
            body,
            "Reset my password\n\nWhat they tried: the reset link\n\nUrgency: high"
        );
        // An entirely empty form keeps a sensible placeholder.
        let empty = DetailForm {
            ticket_id: 1,
            need: "".into(),
            tried: " ".into(),
            urgency: "".into(),
            email: "".into(),
        };
        assert!(compose_ticket_body(&empty).contains("asked for a human"));
    }

    #[test]
    fn strip_leading_quote_drops_only_a_leading_blockquote_header() {
        // The stored support-answer header, then the real answer.
        let body = "> alice: how do I install?\n\nRun the installer, then sign in.";
        assert_eq!(
            strip_leading_quote(body),
            "Run the installer, then sign in."
        );
        // A plain answer with no header is untouched.
        assert_eq!(strip_leading_quote("Just an answer."), "Just an answer.");
        // A blockquote that is the whole (single-paragraph) body is left as-is
        // rather than emptied.
        assert_eq!(strip_leading_quote("> only a quote"), "> only a quote");
    }

    #[test]
    fn parse_source_line_rejects_non_http_and_malformed() {
        assert!(parse_source_line("- product: title (ftp://nope)").is_none());
        assert!(parse_source_line("- no url here").is_none());
        assert!(parse_source_line("not a bullet").is_none());
    }
}
