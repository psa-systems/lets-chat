//! LC-495: workflow-automation engine - the runtime half of the no-code
//! "when X happens in this room, do Y" feature (rule persistence lives in
//! `db::automations`, the config UI in `routes::automations`).
//!
//! Triggers are hooked into the existing event sites: `on_message_posted` is
//! called from the interactive post handler (`routes::room::post_message`) and
//! `on_reaction_added` from the reaction handler. Because the engine is invoked
//! ONLY from those human-facing handlers - never from `finalize_message_send`
//! itself - an action that posts a message cannot re-enter the engine, so rules
//! never cascade or loop. As further belt-and-braces, triggers from bot users
//! are ignored.
//!
//! v1 has two triggers (`message_posted`, `reaction_added`) and one action
//! (`post_message`). The kinds are plain strings end to end so new ones slot in
//! without a schema change; this module is the single place that knows the set.

use crate::db;
use crate::models::User;
use crate::state::AppState;

/// Trigger: a human posted a message. `match_text` is a case-insensitive
/// substring the body must contain.
pub const TRIGGER_MESSAGE_POSTED: &str = "message_posted";
/// Trigger: a human added a reaction. `match_text` is the emoji to match.
pub const TRIGGER_REACTION_ADDED: &str = "reaction_added";
/// Action: post `action_body` (a template) to the room as the automation bot.
pub const ACTION_POST_MESSAGE: &str = "post_message";

/// The shared bot user that automation actions post as.
const AUTOMATION_BOT_USERNAME: &str = "automation";

/// Hard cap on actions fired for a single triggering event, so a room with many
/// matching rules cannot flood on one message.
const MAX_ACTIONS_PER_EVENT: usize = 5;

/// Known trigger kinds (validated on the write path).
pub fn valid_trigger(kind: &str) -> bool {
    matches!(kind, TRIGGER_MESSAGE_POSTED | TRIGGER_REACTION_ADDED)
}

/// Known action kinds (validated on the write path).
pub fn valid_action(kind: &str) -> bool {
    kind == ACTION_POST_MESSAGE
}

/// Does `match_text` match the given trigger `subject`? An empty / whitespace
/// filter matches everything. Message keywords match as a case-insensitive
/// substring; emoji filters match the whole token (also case-insensitively, so
/// shortcode-style emoji like `:tada:` are forgiving).
fn matches_filter(match_text: Option<&str>, subject: &str) -> bool {
    match match_text.map(str::trim) {
        None | Some("") => true,
        Some(needle) => subject.to_lowercase().contains(&needle.to_lowercase()),
    }
}

/// Substitute the supported template variables into an action body.
/// `{user}` -> triggering user's label, `{text}` -> triggering message body,
/// `{emoji}` -> the reaction emoji. Unset variables render empty.
fn render_template(body: &str, user: &str, text: &str, emoji: &str) -> String {
    body.replace("{user}", user)
        .replace("{text}", text)
        .replace("{emoji}", emoji)
}

/// Display label for a user (display name if set, else username).
fn label_of(user: &User) -> &str {
    match user.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n,
        _ => &user.username,
    }
}

/// Resolve (or lazily create) the shared `automation` bot user. Mirrors the
/// assistant-bot pattern in `routes::assistant`, including the create race.
async fn automation_bot(state: &AppState) -> Result<User, crate::error::AppError> {
    use crate::error::AppError;
    if let Some(rec) = db::auth::find_user_by_username(&state.auth, AUTOMATION_BOT_USERNAME).await?
    {
        return Ok(rec.into());
    }
    match db::auth::create_bot(&state.auth, AUTOMATION_BOT_USERNAME).await {
        Ok(id) => db::auth::find_user_by_id(&state.auth, &id)
            .await?
            .map(Into::into)
            .ok_or_else(|| AppError::Internal("automation bot vanished after create".into())),
        Err(sqlx::Error::Database(d)) if d.is_unique_violation() => {
            db::auth::find_user_by_username(&state.auth, AUTOMATION_BOT_USERNAME)
                .await?
                .map(Into::into)
                .ok_or_else(|| AppError::Internal("automation bot vanished after race".into()))
        }
        Err(e) => Err(e.into()),
    }
}

/// Post one rendered action message to `room_id` as the automation bot.
async fn post_action_message(
    state: &AppState,
    room_id: i64,
    body: &str,
) -> Result<(), crate::error::AppError> {
    let body = body.trim();
    if body.is_empty() {
        return Ok(());
    }
    // Honor the same length bound as the composer (LC-153): the body is rendered
    // through `views::markdown` like any message.
    let body: String = body
        .chars()
        .take(crate::routes::room::MAX_MESSAGE_CHARS)
        .collect();
    let room = match db::chat::get_room(&state.chat, room_id).await? {
        Some(r) => r,
        None => return Ok(()),
    };
    let bot = automation_bot(state).await?;
    let new_id = db::chat::insert_message(&state.chat, room_id, &bot.id, &body).await?;
    crate::routes::room::finalize_message_send(state, &room, &bot, new_id, &body, None).await?;
    Ok(())
}

/// Run the `message_posted` trigger for a freshly-posted human message.
/// Best-effort: errors are logged, never surfaced to the poster.
pub async fn on_message_posted(state: &AppState, room_id: i64, author: &User, body: &str) {
    if author.is_bot {
        return; // never let bot posts (incl. our own actions) drive automations
    }
    let rules = match db::automations::list_active_for_trigger(
        &state.chat,
        room_id,
        TRIGGER_MESSAGE_POSTED,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, room_id, "automation: load message rules failed");
            return;
        }
    };
    let user_label = label_of(author);
    let mut fired = 0usize;
    for rule in rules {
        if fired >= MAX_ACTIONS_PER_EVENT {
            break;
        }
        if !matches_filter(rule.match_text.as_deref(), body) {
            continue;
        }
        if rule.action_kind == ACTION_POST_MESSAGE {
            let out = render_template(&rule.action_body, user_label, body, "");
            if let Err(e) = post_action_message(state, room_id, &out).await {
                tracing::warn!(error = %e, room_id, rule_id = rule.id, "automation: post action failed");
            } else {
                fired += 1;
            }
        }
    }
}

/// Run the `reaction_added` trigger when a human adds a reaction.
/// Best-effort: errors are logged, never surfaced.
pub async fn on_reaction_added(state: &AppState, room_id: i64, reactor: &User, emoji: &str) {
    if reactor.is_bot {
        return;
    }
    let rules = match db::automations::list_active_for_trigger(
        &state.chat,
        room_id,
        TRIGGER_REACTION_ADDED,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, room_id, "automation: load reaction rules failed");
            return;
        }
    };
    let user_label = label_of(reactor);
    let mut fired = 0usize;
    for rule in rules {
        if fired >= MAX_ACTIONS_PER_EVENT {
            break;
        }
        if !matches_filter(rule.match_text.as_deref(), emoji) {
            continue;
        }
        if rule.action_kind == ACTION_POST_MESSAGE {
            let out = render_template(&rule.action_body, user_label, "", emoji);
            if let Err(e) = post_action_message(state, room_id, &out).await {
                tracing::warn!(error = %e, room_id, rule_id = rule.id, "automation: post action failed");
            } else {
                fired += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_filter_matches_anything() {
        assert!(matches_filter(None, "anything"));
        assert!(matches_filter(Some(""), "anything"));
        assert!(matches_filter(Some("   "), "anything"));
    }

    #[test]
    fn keyword_filter_is_case_insensitive_substring() {
        assert!(matches_filter(Some("help"), "I need HELP please"));
        assert!(matches_filter(Some("HELP"), "send help"));
        assert!(!matches_filter(Some("help"), "no match here"));
    }

    #[test]
    fn emoji_filter_matches_whole_token() {
        assert!(matches_filter(Some("🎉"), "🎉"));
        assert!(!matches_filter(Some("🎉"), "👍"));
    }

    #[test]
    fn template_substitutes_known_vars_and_blanks_unset() {
        let out = render_template("Hi {user}, you said: {text} {emoji}", "Ann", "yo", "");
        assert_eq!(out, "Hi Ann, you said: yo ");
        let r = render_template("{user} reacted {emoji}", "Bo", "", "🚀");
        assert_eq!(r, "Bo reacted 🚀");
    }

    #[test]
    fn known_kinds() {
        assert!(valid_trigger(TRIGGER_MESSAGE_POSTED));
        assert!(valid_trigger(TRIGGER_REACTION_ADDED));
        assert!(!valid_trigger("nope"));
        assert!(valid_action(ACTION_POST_MESSAGE));
        assert!(!valid_action("webhook"));
    }
}
