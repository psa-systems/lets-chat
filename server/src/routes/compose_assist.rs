//! LC-532: composer AI writing assistant. Sends the current draft to the
//! operator's LLM with a rewrite prompt and returns a suggestion the user can
//! apply into the composer. Mirrors `routes::translate` (gate on `llm_client`,
//! room access, one `complete()` call). The draft leaves the device only on an
//! explicit click, and only when an LLM is configured.

use axum::extract::{Path, State};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
#[allow(unused_imports)]
use crate::i18n::filters;
use crate::state::AppState;
use crate::views::{html, Html};
use askama::Template; // LC-188: in-scope for the |t template filter.

/// Cap on the draft length sent to the LLM. Rewriting a wall of text is
/// pointless and costly; the composer's own message cap is far larger.
const MAX_ASSIST_CHARS: usize = 4000;

/// The rewrite modes offered in the panel (wire values). The default is the
/// first entry.
const ACTIONS: &[&str] = &["rephrase", "grammar", "concise", "friendly", "formal"];

fn resolve_action(a: &str) -> &'static str {
    ACTIONS
        .iter()
        .copied()
        .find(|x| *x == a)
        .unwrap_or("rephrase")
}

/// System prompt for a rewrite action. Every mode preserves meaning, language,
/// markdown, code, URLs, and @mentions, and replies with ONLY the rewrite.
fn system_prompt(action: &str) -> String {
    let clause = match action {
        "grammar" => "Fix only spelling, grammar, and punctuation; keep the wording and tone as close to the original as possible.",
        "concise" => "Rewrite it to be more concise and to the point.",
        "friendly" => "Rewrite it in a warmer, friendlier tone.",
        "formal" => "Rewrite it in a more formal, professional tone.",
        _ => "Rewrite it to be clearer and more polished.",
    };
    format!(
        "You are a writing assistant for a chat app. {clause} Preserve the original meaning and \
         language. Keep markdown, code spans, URLs, and @mentions unchanged. Reply with ONLY the \
         rewritten message - no preamble, notes, or surrounding quotes."
    )
}

#[derive(Deserialize)]
pub struct AssistForm {
    pub body: String,
    #[serde(default)]
    pub action: String,
}

#[derive(Template)]
#[template(path = "room/compose_assist_panel.html")]
struct AssistPanel {
    room_id: i64,
    suggestion: String,
    /// LC-655: the mode this result was produced with, so the header can name
    /// it and the Regenerate button can re-run the same mode. The mode menu now
    /// lives on the composer's sparkle button (composer.html), not in the panel.
    active_action: &'static str,
    active_label: String,
}

/// POST /room/{room_id}/compose-assist
pub async fn post_assist(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(room_id): Path<i64>,
    Form(form): Form<AssistForm>,
) -> Result<Html, AppError> {
    let Some(llm) = state.llm_client.clone() else {
        return Err(AppError::BadRequest(
            "the writing assistant is not configured".into(),
        ));
    };
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }

    let action = resolve_action(form.action.trim());
    let draft: String = form.body.trim().chars().take(MAX_ASSIST_CHARS).collect();
    if draft.is_empty() {
        return Err(AppError::BadRequest("type a message first".into()));
    }

    let suggestion = match llm.complete_guarded(&system_prompt(action), &draft).await {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            tracing::warn!(error = %e, "compose-assist failed");
            return Err(AppError::BadRequest("the writing assistant failed".into()));
        }
    };
    if suggestion.is_empty() {
        return Err(AppError::BadRequest(
            "the writing assistant returned nothing".into(),
        ));
    }

    let active_label = crate::i18n::translate_current(&format!("compose-assist-action-{action}"));
    html(&AssistPanel {
        room_id,
        suggestion,
        active_action: action,
        active_label,
    })
}

#[cfg(test)]
mod tests {
    use super::{resolve_action, system_prompt};

    #[test]
    fn unknown_action_falls_back_to_rephrase() {
        assert_eq!(resolve_action("bogus"), "rephrase");
        assert_eq!(resolve_action(""), "rephrase");
        assert_eq!(resolve_action("formal"), "formal");
    }

    #[test]
    fn prompts_differ_by_action_and_forbid_preamble() {
        let g = system_prompt("grammar");
        let f = system_prompt("formal");
        assert!(g.contains("grammar"));
        assert!(f.contains("formal"));
        for a in ["rephrase", "grammar", "concise", "friendly", "formal"] {
            assert!(system_prompt(a).contains("ONLY the"));
        }
    }
}
