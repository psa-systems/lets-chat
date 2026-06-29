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

/// The assistant's bot username (and display label).
const ASSISTANT_USERNAME: &str = "assistant";

const SYSTEM_PROMPT: &str = "You are a helpful assistant inside a team chat room. \
Answer the user's question using ONLY the provided context (recent room messages, \
the room's wiki, and its description). If the answer is not in the context, say you \
could not find it in this room rather than guessing. Be concise and use Markdown. \
Do not invent facts, links, or quotes.";

/// Resolve (or lazily create) the shared `assistant` bot user and return it as
/// a `User` suitable for `finalize_message_send`.
async fn assistant_bot(state: &AppState) -> Result<User, AppError> {
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

    // FTS over the room's messages. A query that sanitizes to nothing (only
    // stop-symbols) just yields no message context; the wiki/description still
    // ground the answer.
    if let Some(fts) = db::chat::sanitize_fts_query(question) {
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

    let context = build_context(state, room, question).await?;
    let user_content = format!("Context:\n{context}\n\nQuestion: {question}");
    let answer = match llm.complete(SYSTEM_PROMPT, &user_content).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, room_id = room.id, "assistant LLM call failed");
            return Err(AppError::BadRequest(
                "The assistant could not answer right now. Try again later.".into(),
            ));
        }
    };
    let answer = answer.trim();
    if answer.is_empty() {
        return Err(AppError::BadRequest(
            "The assistant returned an empty answer.".into(),
        ));
    }

    // Post as the assistant bot, quoting the asker's question for context (the
    // `/ask` itself is not echoed into the room).
    let bot = assistant_bot(state).await?;
    let asker_label = match asker.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n,
        _ => &asker.username,
    };
    let body = format!("> **{asker_label} asked:** {question}\n\n{answer}");
    let new_id = db::chat::insert_message(&state.chat, room.id, &bot.id, &body).await?;
    super::room::finalize_message_send(state, room, &bot, new_id, &body, None).await?;
    Ok(())
}
