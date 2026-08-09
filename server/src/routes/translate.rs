//! LC-486: per-message "translate to my language" via the operator LLM.
//!
//! Reuses the LLM `complete` path (LC-396/LC-484) with a translation prompt,
//! targets the viewer's active locale, and caches the result keyed by
//! (message_id, locale) so repeat opens + viewers sharing a locale are free.
//! The edit path invalidates the cache (see routes::room::patch_message).

use axum::extract::{Path, State};
use axum::Form;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::translate::{resolve_target, TranslationFragment, TARGETS};
use crate::views::{html, Html};

/// LC-688: the picker posts the chosen target language here (the `<select>`'s
/// `name="lang"`). The initial Translate menu click posts no body, so `lang` is
/// absent and the target defaults to the viewer's locale.
#[derive(Deserialize, Default)]
pub struct TranslateForm {
    #[serde(default)]
    pub lang: Option<String>,
}

/// POST /messages/{message_id}/translate -> render the message translated into
/// the chosen target language (LC-688), defaulting to the viewer's locale, and
/// cached per (message, target).
pub async fn post_translate(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
    // The initial Translate menu click posts an empty body; an all-optional form
    // deserializes that to `lang: None` (no error), so no `Option` wrapper needed.
    Form(form): Form<TranslateForm>,
) -> Result<Html, AppError> {
    let Some(llm) = state.llm_client.clone() else {
        return Err(AppError::BadRequest("translation is not configured".into()));
    };
    let msg = db::chat::get_message(&state.chat, message_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let is_admin = user.role == "admin";
    if !db::chat::is_room_accessible(&state.chat, msg.room_id, &user.id, is_admin).await? {
        return Err(AppError::Forbidden);
    }
    // LC-679: runtime flag + role gate (403 for unprivileged/flag-off).
    super::ai_gate::require_llm_in_room(&state, msg.room_id, &user).await?;
    let body = msg.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("nothing to translate".into()));
    }

    // LC-688: target language is the picker's choice, defaulting to the viewer's
    // locale; the cache is keyed by the target code so each language caches
    // independently.
    let requested = form.lang;
    let viewer_locale = crate::i18n::current_lang_code();
    let target = resolve_target(requested.as_deref(), &viewer_locale);

    let translated =
        match db::translations::get_cached(&state.chat, message_id, target.code).await? {
            Some(cached) => cached,
            None => {
                let lang = target.english;
                let system = format!(
                    "You are a translation engine. Translate the user's chat message into {lang}. \
                 Preserve markdown, code spans, URLs, and @mentions unchanged. If the message is \
                 already in {lang}, return it unchanged. Reply with ONLY the translation - no \
                 preamble, notes, or surrounding quotes."
                );
                let out = match llm.complete_guarded(&system, body).await {
                    Ok(s) => s.trim().to_string(),
                    Err(e) => {
                        tracing::warn!(error = %e, message_id, "translation failed");
                        return Err(AppError::BadRequest("translation failed".into()));
                    }
                };
                if out.is_empty() {
                    return Err(AppError::BadRequest("translation failed".into()));
                }
                db::translations::upsert(&state.chat, message_id, target.code, &out).await?;
                out
            }
        };

    let translated_html = crate::views::markdown::render(&translated, &[], &[]);
    html(&TranslationFragment {
        message_id,
        translated_html,
        selected: target.code,
        targets: TARGETS,
    })
}
