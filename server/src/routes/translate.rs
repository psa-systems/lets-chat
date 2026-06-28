//! LC-486: per-message "translate to my language" via the operator LLM.
//!
//! Reuses the LLM `complete` path (LC-396/LC-484) with a translation prompt,
//! targets the viewer's active locale, and caches the result keyed by
//! (message_id, locale) so repeat opens + viewers sharing a locale are free.
//! The edit path invalidates the cache (see routes::room::patch_message).

use axum::extract::{Path, State};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::translate::TranslationFragment;
use crate::views::{html, Html};

/// POST /messages/{message_id}/translate -> render the message translated into
/// the viewer's locale (cached).
pub async fn post_translate(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(message_id): Path<i64>,
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
    let body = msg.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("nothing to translate".into()));
    }

    let locale = crate::i18n::current_lang_code();

    let translated = match db::translations::get_cached(&state.chat, message_id, &locale).await? {
        Some(cached) => cached,
        None => {
            let lang = crate::i18n::language_name(&locale);
            let system = format!(
                "You are a translation engine. Translate the user's chat message into {lang}. \
                 Preserve markdown, code spans, URLs, and @mentions unchanged. If the message is \
                 already in {lang}, return it unchanged. Reply with ONLY the translation - no \
                 preamble, notes, or surrounding quotes."
            );
            let out = match llm.complete(&system, body).await {
                Ok(s) => s.trim().to_string(),
                Err(e) => {
                    tracing::warn!(error = %e, message_id, "translation failed");
                    return Err(AppError::BadRequest("translation failed".into()));
                }
            };
            if out.is_empty() {
                return Err(AppError::BadRequest("translation failed".into()));
            }
            db::translations::upsert(&state.chat, message_id, &locale, &out).await?;
            out
        }
    };

    let translated_html = crate::views::markdown::render(&translated, &[], &[]);
    html(&TranslationFragment {
        message_id,
        translated_html,
        language: crate::i18n::native_name(&locale).to_string(),
    })
}
