//! LC-492: in-channel AI assistant (`/ask`). A RAG question-answerer that
//! retrieves context from the room (FTS over messages + the room wiki /
//! description) and answers with the operator-configured LLM
//! (`crate::llm`), posting the reply as the `assistant` bot user.
//!
//! Gated three ways: the operator must have configured an LLM, the room must
//! have the assistant enabled (`rooms.assistant_enabled`, a manager opt-in),
//! and the asker is rate-limited. The dispatcher (`routes::slash`) has already
//! enforced room access + posting policy before this runs, so retrieval skips
//! re-scoping.

use std::collections::HashMap;

use crate::db;
use crate::error::AppError;
use crate::models::{Room, User};
use crate::rate_limit::{Outcome, RateLimitKind};
use crate::state::AppState;

/// Max characters accepted in a question (bounds the prompt + abuse).
const MAX_QUESTION_CHARS: usize = 1000;
/// Top-N FTS messages pulled into the retrieval context.
const RETRIEVAL_LIMIT: i64 = 12;
/// Per-message body cap inside the context block (keeps one long message from
/// crowding out the rest).
const SNIPPET_CHARS: usize = 500;
/// Overall cap on the assembled context fed to the model.
const CONTEXT_CHARS: usize = 8000;
/// Per-user asks per minute.
const ASK_RATE_PER_MIN: u32 = 10;

/// The assistant's default bot username (and display label). Used when no bot
/// has been chosen on the admin Bots page, and as the lazily-created fallback.
const ASSISTANT_USERNAME: &str = "assistant";

/// LC-693: settings key naming the bot the AI assistant posts as. An admin picks
/// it on the Bots page from the existing bot accounts; unset (or a stale /
/// disabled choice) falls back to [`ASSISTANT_USERNAME`].
pub(crate) const ASSISTANT_BOT_KEY: &str = "assistant_bot_username";

/// LC-695: settings key prefix for a bot's custom `/ask` persona (the personality
/// half of its system prompt), keyed by the bot's username. Empty/absent means
/// the default persona. Stored in the settings KV, like [`ASSISTANT_BOT_KEY`].
pub(crate) const BOT_PERSONA_KEY_PREFIX: &str = "bot_persona:";

/// The settings key holding `username`'s custom `/ask` persona.
pub(crate) fn bot_persona_key(username: &str) -> String {
    format!("{BOT_PERSONA_KEY_PREFIX}{username}")
}

/// LC-695: the persona (role / personality) half of the `/ask` system prompt.
/// An admin can override this per assistant bot; the invariant [`ASSISTANT_RULES`]
/// half is always appended, so a custom persona can shape tone and role but can
/// never remove the safety refusal or the "answer from context" contract.
const DEFAULT_PERSONA: &str = "You are a helpful assistant inside a team chat room.";

// LC-676: keep the RAG contract (answer from the room's context) but (a) make a
// no-context outcome helpful instead of a terse dead-end, and (b) refuse a
// harmful request on SAFETY grounds rather than phrasing it as a retrieval miss
// ("could not find information on making bombs in this room" implied it would
// answer if the room had it).
// LC-695: this is the INVARIANT half of the prompt - always applied, whether the
// persona is the default or a bot's custom one, so safety + the retrieval
// contract cannot be prompted away.
const ASSISTANT_RULES: &str = "Answer the user's \
question using the provided context (recent room messages, the room's wiki, and its description). \
If the context does not contain the answer, say so plainly and helpfully - for example \"I couldn't \
find anything in this room about X. It may not have been discussed here - try rephrasing or asking \
in a different channel.\" - and do NOT imply you would have answered if the room had contained it. \
If the question asks for harmful, dangerous, illegal, or otherwise unsafe information, refuse briefly \
and plainly on safety grounds, regardless of the context, and never phrase such a refusal as a \
retrieval miss. Be concise and use Markdown. Do not invent facts, links, or quotes.";

/// LC-695: compose the effective `/ask` system prompt. A non-empty `persona`
/// (a bot's custom instructions) replaces the default personality; the invariant
/// [`ASSISTANT_RULES`] envelope (safety + retrieval contract + formatting) is
/// always appended and cannot be overridden. A blank persona falls back to the
/// default, matching how the identity falls back to the built-in `assistant`.
fn effective_system_prompt(persona: Option<&str>) -> String {
    let persona = persona
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(DEFAULT_PERSONA);
    format!("{persona}\n\n{ASSISTANT_RULES}")
}

/// LC-676: placeholder shown the instant an /ask is sent, then replaced in place
/// by the answer. Italic so it reads as a transient status, not a real answer.
const THINKING_BODY: &str = "_The assistant is thinking\u{2026}_";

/// Resolve (or lazily create) the shared `assistant` bot user and return it as
/// a `User` suitable for `finalize_message_send`. LC-662: also used by the
/// automatic post-call recap (`routes::transcripts`), which posts as the same bot.
pub(crate) async fn assistant_bot(state: &AppState) -> Result<User, AppError> {
    // LC-693: an admin can choose which bot the AI assistant posts as (settings
    // `assistant_bot_username`, set from the admin Bots page). Honor it only when
    // it names an existing, active bot; an unset, renamed, or disabled choice
    // falls through to the built-in `assistant` identity below, so /ask and the
    // other assistant surfaces never break on a stale setting.
    if let Some(chosen) = db::settings::get_setting(&state.settings, ASSISTANT_BOT_KEY).await? {
        let chosen = chosen.trim();
        if !chosen.is_empty() && chosen != ASSISTANT_USERNAME {
            if let Some(rec) = db::auth::find_user_by_username(&state.auth, chosen).await? {
                if rec.is_bot && !rec.is_banned {
                    return Ok(rec.into());
                }
            }
        }
    }
    if let Some(rec) = db::auth::find_user_by_username(&state.auth, ASSISTANT_USERNAME).await? {
        return Ok(rec.into());
    }
    // Race: a concurrent first-ask may create it between the lookup and here;
    // fall back to a re-lookup on a unique-constraint violation.
    match db::auth::create_bot(&state.auth, ASSISTANT_USERNAME).await {
        Ok(id) => db::auth::find_user_by_id(&state.auth, &id)
            .await?
            .map(Into::into)
            .ok_or_else(|| AppError::Internal("assistant bot vanished after create".into())),
        Err(sqlx::Error::Database(d)) if d.is_unique_violation() => {
            db::auth::find_user_by_username(&state.auth, ASSISTANT_USERNAME)
                .await?
                .map(Into::into)
                .ok_or_else(|| AppError::Internal("assistant bot vanished after race".into()))
        }
        Err(e) => Err(e.into()),
    }
}

/// LC-695: the username of the bot the assistant currently posts as, WITHOUT
/// lazily creating the built-in `assistant` (unlike [`assistant_bot`]) - so
/// merely rendering the admin Bots page does not conjure the bot. Mirrors the
/// same selection logic: the chosen active bot, else `assistant`. Used to key a
/// bot's custom `/ask` persona ([`bot_persona_key`]).
pub(crate) async fn effective_assistant_username(state: &AppState) -> Result<String, AppError> {
    if let Some(chosen) = db::settings::get_setting(&state.settings, ASSISTANT_BOT_KEY).await? {
        let chosen = chosen.trim();
        if !chosen.is_empty() && chosen != ASSISTANT_USERNAME {
            if let Some(rec) = db::auth::find_user_by_username(&state.auth, chosen).await? {
                if rec.is_bot && !rec.is_banned {
                    return Ok(rec.username);
                }
            }
        }
    }
    Ok(ASSISTANT_USERNAME.to_string())
}

/// Build the retrieval context block for `question` in `room`: the room wiki +
/// description, then the FTS-ranked relevant messages as `Label: body` lines.
async fn build_context(state: &AppState, room: &Room, question: &str) -> Result<String, AppError> {
    let mut ctx = String::new();

    if let Some(desc) = room.description.as_deref().map(str::trim) {
        if !desc.is_empty() {
            ctx.push_str("Room description:\n");
            ctx.push_str(desc);
            ctx.push_str("\n\n");
        }
    }
    if let Some(wiki) = room.wiki_body.as_deref().map(str::trim) {
        if !wiki.is_empty() {
            ctx.push_str("Room wiki:\n");
            ctx.push_str(wiki);
            ctx.push_str("\n\n");
        }
    }

    // FTS over the room's messages. LC-676: a natural-language question needs
    // ANY-term (OR) retrieval, not the search box's AND, or "who is david?"
    // matches nothing even when the room is full of David. A query that
    // sanitizes to nothing (only stop-symbols) just yields no message context;
    // the wiki/description still ground the answer.
    if let Some(fts) = db::chat::fts_query_any(question) {
        let hits = db::chat::fts_room_context(&state.chat, room.id, &fts, RETRIEVAL_LIMIT).await?;
        if !hits.is_empty() {
            let author_ids: Vec<&str> = hits
                .iter()
                .map(|(uid, _)| uid.as_str())
                .collect::<std::collections::HashSet<&str>>()
                .into_iter()
                .collect();
            let labels = db::auth::display_names_for_ids(&state.auth, &author_ids)
                .await
                .unwrap_or_default();
            ctx.push_str("Relevant messages:\n");
            for (uid, body) in hits {
                let label = label_for(&labels, &uid);
                let snippet: String = body.chars().take(SNIPPET_CHARS).collect();
                ctx.push_str(&format!("{label}: {snippet}\n"));
            }
        }
    }

    Ok(ctx.chars().take(CONTEXT_CHARS).collect())
}

fn label_for(labels: &HashMap<String, (String, Option<String>)>, uid: &str) -> String {
    match labels.get(uid) {
        Some((username, display_name)) => match display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n.to_string(),
            _ => username.clone(),
        },
        None => uid.to_string(),
    }
}

/// Handle `/ask <question>` for `asker` in `room`. Retrieves context, calls the
/// LLM, and posts the answer as the `assistant` bot. Errors surface to the
/// asker's composer (the slash command runs `hx-swap="none"`); nothing is
/// posted on failure.
pub(crate) async fn handle_ask(
    state: &AppState,
    room: &Room,
    asker: &User,
    question: &str,
) -> Result<(), AppError> {
    let Some(llm) = state.llm_client.clone() else {
        return Err(AppError::BadRequest(
            "The AI assistant is not configured on this server.".into(),
        ));
    };
    // LC-679: runtime flag + role gate. 403 for an unprivileged/flag-off caller
    // so /ask is refused on the server, not merely hidden from the slash menu.
    super::ai_gate::require_llm_in_room(state, room.id, asker).await?;
    if !db::chat::get_room_assistant_enabled(&state.chat, room.id).await? {
        return Err(AppError::BadRequest(
            "The AI assistant is not enabled in this room.".into(),
        ));
    }
    let question = question.trim();
    if question.is_empty() {
        return Err(AppError::BadRequest("usage: /ask <question>".into()));
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        return Err(AppError::BadRequest(
            "That question is too long for the assistant.".into(),
        ));
    }
    if let Outcome::Deny { .. } =
        state
            .rate_limits
            .check(RateLimitKind::AssistantAsk, &asker.id, ASK_RATE_PER_MIN)
    {
        return Err(AppError::BadRequest(
            "You're asking the assistant too quickly. Try again in a minute.".into(),
        ));
    }

    // LC-676 #3: a quiet, compact attribution line (a reply-style quote) that
    // stays subordinate to the answer, instead of a heavy bold blockquote.
    let asker_label = match asker.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n,
        _ => &asker.username,
    };
    let header = format!("> {asker_label}: {question}");

    // LC-676 #1: post a "thinking..." placeholder as the bot and broadcast it
    // immediately, so the asker sees the assistant working right away.
    let bot = assistant_bot(state).await?;
    // LC-695: the active bot's custom persona (if any) shapes the /ask system
    // prompt; the safety envelope is always applied by effective_system_prompt.
    let persona =
        db::settings::get_setting(&state.settings, &bot_persona_key(&bot.username)).await?;
    let system_prompt = effective_system_prompt(persona.as_deref());
    let placeholder = format!("{header}\n\n{THINKING_BODY}");
    let msg_id = db::chat::insert_message(&state.chat, room.id, &bot.id, &placeholder).await?;
    super::room::finalize_message_send(state, room, &bot, msg_id, &placeholder, None).await?;

    // Run the (multi-second) LLM call OFF the request path so the /ask POST
    // returns now. Otherwise it holds open for the whole answer, and the
    // composer - which clears its text + slash hint on the POST's return - stays
    // full and the hint hangs until then. The placeholder is replaced in place
    // when the answer is ready (silent update: no "(edited)" marker).
    let st = state.clone();
    let room = room.clone();
    let question = question.to_string();
    tokio::spawn(async move {
        let final_body =
            build_ask_answer(&st, &room, &question, &header, &*llm, &system_prompt).await;
        if let Err(e) = db::chat::replace_message_body(&st.chat, msg_id, &final_body).await {
            tracing::warn!(error = %e, message_id = msg_id, "failed to store assistant answer");
            return;
        }
        st.hub.broadcast_to_room(
            room.id,
            &crate::ws::events::ChatEvent::MessageEdited {
                message_id: msg_id,
                room_id: room.id,
                new_body: final_body,
                edited_at: String::new(),
            },
        );
    });
    Ok(())
}

/// LC-676: retrieve context, ask the LLM, and format the final answer body
/// (keeping the quiet `header` attribution line). Returns a friendly, italic
/// error body on any failure so the "thinking..." placeholder always resolves.
async fn build_ask_answer(
    state: &AppState,
    room: &Room,
    question: &str,
    header: &str,
    llm: &dyn crate::llm::LlmClient,
    system_prompt: &str,
) -> String {
    let context = match build_context(state, room, question).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, room_id = room.id, "assistant context build failed");
            return format!(
                "{header}\n\n_The assistant could not answer right now. Try again later._"
            );
        }
    };
    let user_content = format!("Context:\n{context}\n\nQuestion: {question}");
    match llm.complete_guarded(system_prompt, &user_content).await {
        Ok(a) if !a.trim().is_empty() => format!("{header}\n\n{}", a.trim()),
        Ok(_) => format!("{header}\n\n_The assistant returned an empty answer._"),
        Err(e) => {
            tracing::warn!(error = %e, room_id = room.id, "assistant LLM call failed");
            format!("{header}\n\n_The assistant could not answer right now. Try again later._")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{effective_system_prompt, DEFAULT_PERSONA};

    #[test]
    fn ask_prompt_is_helpful_on_no_context_and_refuses_unsafe() {
        let lo = effective_system_prompt(None).to_lowercase();
        // LC-676 #2a: a no-context outcome is helpful, not a terse dead-end, and
        // does not imply it would have answered.
        assert!(lo.contains("does not contain the answer"));
        assert!(lo.contains("do not imply"));
        // LC-676 #2b: an explicit safety refusal, never phrased as a retrieval miss.
        assert!(lo.contains("safety grounds"));
        assert!(lo.contains("never phrase such a refusal as a retrieval miss"));
        // Still RAG-grounded + no hallucination.
        assert!(lo.contains("provided context"));
        assert!(lo.contains("do not invent"));
        // Default persona opener is present.
        assert!(lo.contains("helpful assistant"));
    }

    #[test]
    fn custom_persona_shapes_role_but_cannot_drop_the_safety_envelope() {
        // LC-695: a custom persona replaces the personality opener...
        let p = effective_system_prompt(Some("You are a terse pirate stand-up bot."));
        let lo = p.to_lowercase();
        assert!(lo.contains("pirate")); // persona applied
        assert!(!lo.contains("helpful assistant")); // default opener replaced
                                                    // ...but the invariant rules (safety + retrieval contract) always apply.
        assert!(lo.contains("safety grounds"));
        assert!(lo.contains("never phrase such a refusal as a retrieval miss"));
        assert!(lo.contains("provided context"));
        assert!(lo.contains("do not invent"));
    }

    #[test]
    fn blank_persona_falls_back_to_the_default() {
        assert_eq!(
            effective_system_prompt(Some("   ")),
            effective_system_prompt(None)
        );
        assert!(effective_system_prompt(None).starts_with(DEFAULT_PERSONA));
    }
}
