//! LC-780: viewer-independent render context for per-recipient WebSocket renders.
//!
//! Every connected recipient of a message event runs its own render task. The
//! producer of the event used to hand them only the message (or just its id),
//! so each recipient re-queried the author, attachments, mentions, grouping
//! predecessor and quote preview, all identical for every viewer. The producer
//! now resolves that once, wraps it in an `Arc` and carries it on the event, so
//! the per-recipient clone is a refcount bump. A recipient still does the work
//! that genuinely varies by viewer: the block and moderator checks, its personal
//! emoji, the `#channel` links its access allows, and the template render.

use std::sync::Arc;

use crate::db;
use crate::db::mentions::MentionRef;
use crate::error::AppError;
use crate::models::custom_emoji::EmojiRef;
use crate::models::reaction::Reaction;
use crate::models::{Attachment, Message, Room};
use crate::state::AppState;
use crate::views::room::QuotePreview;

use super::AuthorMeta;

/// The viewer-independent pieces every message render shares.
#[derive(Debug, Clone)]
pub struct MessageRenderBase {
    /// The author with their PERSISTED status; see [`Self::author_for`].
    pub author: AuthorMeta,
    /// True when the author is a real user. Only a real user's presence is
    /// viewer-dependent; webhook, email and bridge actors are always active.
    pub author_is_user: bool,
    pub attachments: Vec<Attachment>,
    pub mentions: Vec<MentionRef>,
    pub quote_preview: Option<QuotePreview>,
    /// The room's enclave, so each recipient skips that lookup when it lists the
    /// `#channel` links its access allows.
    pub enclave_id: Option<i64>,
    /// The room's custom emoji set. A recipient layers its personal set on top.
    pub room_emojis: Vec<EmojiRef>,
    /// Whether the message is a poll. A message with no poll renders none for
    /// every viewer, so only a poll message pays the per-viewer view build.
    pub has_poll: bool,
    /// Whether the message is a follow-up list; same reasoning as `has_poll`.
    pub has_follow_up: bool,
}

impl MessageRenderBase {
    async fn load(state: &AppState, m: &Message) -> Result<Self, AppError> {
        let (author, author_is_user) = super::resolve_msg_author_persisted(
            state,
            &m.user_id,
            m.webhook_id,
            m.email_inbox_id,
            m.bridge_id,
            m.bridge_foreign_name.as_deref(),
            m.bridge_kind.as_deref(),
            m.bridge_foreign_avatar.as_deref(),
            m.room_id,
        )
        .await?;
        let attachments = db::uploads::attachments_for_message(&state.chat, m.id).await?;
        let mentions = db::mentions::mentions_for_messages(&state.chat, &state.auth, &[m.id])
            .await?
            .remove(&m.id)
            .unwrap_or_default();
        let quote_preview = match m.quote_id {
            Some(qid) => {
                crate::views::room::build_quote_preview(&state.chat, &state.auth, qid).await?
            }
            None => None,
        };
        let enclave_id = super::enclave_for_room(state, m.room_id).await?;
        let room_emojis = db::custom_emojis::refs_for_room(&state.chat, m.room_id).await?;
        let has_poll = db::polls::get(&state.chat, m.id).await?.is_some();
        let has_follow_up = db::followups::get(&state.chat, m.id).await?.is_some();
        Ok(Self {
            author,
            author_is_user,
            attachments,
            mentions,
            quote_preview,
            enclave_id,
            room_emojis,
            has_poll,
            has_follow_up,
        })
    }

    /// The author as `viewer_id` sees them.
    pub fn author_for(
        &self,
        state: &AppState,
        author_user_id: &str,
        viewer_id: &str,
    ) -> AuthorMeta {
        if self.author_is_user {
            self.author
                .clone()
                .for_viewer(state, author_user_id, viewer_id)
        } else {
            self.author.clone()
        }
    }

    /// The emoji set `viewer_id` sees: the shared room set with their personal
    /// set layered under it (same precedence as
    /// `db::custom_emojis::refs_for_room_and_user`).
    pub async fn custom_emojis_for(
        &self,
        state: &AppState,
        viewer_id: &str,
    ) -> Result<Vec<EmojiRef>, sqlx::Error> {
        let personal = db::custom_emojis::refs_for_user(&state.chat, viewer_id).await?;
        Ok(db::custom_emojis::merge_room_and_personal(
            self.room_emojis.clone(),
            personal,
        ))
    }

    /// The poll block as `viewer_id` sees it (their own votes); `None` without
    /// a query when the message is not a poll.
    pub async fn poll_for(
        &self,
        state: &AppState,
        message_id: i64,
        viewer_id: &str,
    ) -> Option<crate::views::room::PollView> {
        if !self.has_poll {
            return None;
        }
        crate::views::room::optional_block(
            crate::views::room::build_poll_view(&state.chat, &state.auth, message_id, viewer_id)
                .await,
            message_id,
            "poll",
        )
    }

    /// The follow-up list as `viewer_id` sees it (their own assignments);
    /// `None` without a query when the message is not a follow-up list.
    pub async fn follow_up_for(
        &self,
        state: &AppState,
        message_id: i64,
        viewer_id: &str,
    ) -> Option<crate::views::room::FollowUpView> {
        if !self.has_follow_up {
            return None;
        }
        crate::views::room::optional_block(
            crate::views::room::build_follow_up_view(
                &state.chat,
                &state.auth,
                message_id,
                viewer_id,
            )
            .await,
            message_id,
            "follow_up",
        )
    }

    /// The `#channel` link targets `viewer` may see in this message's enclave.
    pub async fn channels_for(
        &self,
        state: &AppState,
        viewer: &crate::models::User,
    ) -> Result<Vec<crate::views::channel_complete::ChannelRef>, AppError> {
        match self.enclave_id {
            Some(enclave_id) => super::channel_refs_for_enclave(state, enclave_id, viewer).await,
            None => Ok(Vec::new()),
        }
    }
}

/// Context carried by `ChatEvent::NewMessage`.
#[derive(Debug, Clone)]
pub struct NewMessageContext {
    pub base: MessageRenderBase,
    /// Whether the message groups under the room's previous message. Neither
    /// input depends on the viewer.
    pub is_follow_up: bool,
    /// The room, already loaded by the producer; the seen-bar and the sidebar
    /// unread bump both need it.
    pub room: Room,
}

impl NewMessageContext {
    pub async fn load(
        state: &AppState,
        message: &Message,
        room: &Room,
    ) -> Result<Arc<Self>, AppError> {
        let base = MessageRenderBase::load(state, message).await?;
        let prior =
            db::chat::prior_message_in_room(&state.chat, message.room_id, message.id).await?;
        let is_follow_up = db::chat::is_follow_up_of(
            prior
                .as_ref()
                .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
            (message.user_id.as_str(), message.created_at.as_str()),
        );
        Ok(Arc::new(Self {
            base,
            is_follow_up,
            room: room.clone(),
        }))
    }
}

/// Context carried by the events that re-render an existing message
/// (`MessageEdited`, `MessageRegrouped`, `VoiceTranscribed`,
/// `AttachmentAltChanged`). Built from the database at event time, so it is as
/// canonical as the per-recipient re-fetch it replaces.
#[derive(Debug, Clone)]
pub struct EditedMessageContext {
    pub base: MessageRenderBase,
    /// The message as stored; `author_name` is the resolved author username.
    pub message: Message,
    pub is_follow_up: bool,
    pub reply_count: i64,
    pub is_pinned: bool,
    /// Reactions with every reactor id; a recipient derives its own
    /// `reacted_by_me` from them.
    pub reactions: Vec<Reaction>,
    /// The "who reacted" tooltip text, aligned to `reactions`.
    pub reactor_titles: Vec<String>,
}

impl EditedMessageContext {
    /// Producer helper for the best-effort live update: the edit, transcript or
    /// alt text is already stored, so a failure here must not fail the request,
    /// but it must be findable. Returns the context to broadcast, or `None`
    /// (logged, naming the message and event) when there is nothing to send.
    pub async fn load_for_broadcast(
        state: &AppState,
        message_id: i64,
        event: &'static str,
    ) -> Option<Arc<Self>> {
        match Self::load(state, message_id).await {
            Ok(Some(ctx)) => Some(ctx),
            Ok(None) => {
                tracing::info!(
                    message_id,
                    event,
                    "message is gone; no live re-render to broadcast"
                );
                None
            }
            Err(e) => {
                tracing::error!(error = %e, message_id, event, "failed to resolve render context; recipients will not see this update live");
                None
            }
        }
    }

    /// `Ok(None)` when the message no longer exists (deleted or quarantined):
    /// there is nothing to re-render for anyone.
    pub async fn load(state: &AppState, message_id: i64) -> Result<Option<Arc<Self>>, AppError> {
        let Some(raw) = db::chat::get_message(&state.chat, message_id).await? else {
            return Ok(None);
        };
        let mut message = Message {
            id: raw.id,
            room_id: raw.room_id,
            user_id: raw.user_id,
            author_name: String::new(),
            body: raw.body,
            created_at: raw.created_at,
            edited_at: raw.edited_at,
            parent_id: raw.parent_id,
            quote_id: raw.quote_id,
            is_system: raw.is_system,
            webhook_id: raw.webhook_id,
            email_inbox_id: raw.email_inbox_id,
            bridge_id: raw.bridge_id,
            bridge_foreign_name: raw.bridge_foreign_name,
            bridge_kind: raw.bridge_kind,
            bridge_foreign_avatar: raw.bridge_foreign_avatar,
        };
        let base = MessageRenderBase::load(state, &message).await?;
        message.author_name = base.author.username.clone();
        let reactions = db::chat::list_reactions(&state.chat, message.id, "").await?;
        let reactor_titles = super::build_reactor_titles(state, &reactions).await;
        let prior =
            db::chat::prior_message_in_room(&state.chat, message.room_id, message.id).await?;
        let is_follow_up = db::chat::is_follow_up_of(
            prior
                .as_ref()
                .map(|p| (p.user_id.as_str(), p.created_at.as_str())),
            (message.user_id.as_str(), message.created_at.as_str()),
        );
        let reply_count = db::chat::count_replies(&state.chat, message.id).await?;
        let is_pinned = db::pinned::pinned_message_ids_for_room(&state.chat, message.room_id)
            .await?
            .contains(&message.id);
        Ok(Some(Arc::new(Self {
            base,
            message,
            is_follow_up,
            reply_count,
            is_pinned,
            reactions,
            reactor_titles,
        })))
    }
}

/// Context carried by `ChatEvent::ThreadReply`.
#[derive(Debug, Clone)]
pub struct ThreadReplyContext {
    pub base: MessageRenderBase,
    /// The previous reply in the thread as `(user_id, created_at)`. Whether it
    /// groups the new reply is per viewer: a predecessor the viewer has blocked
    /// is invisible to them.
    pub prior: Option<(String, String)>,
    /// The parent's reply count after this reply, for the count pill.
    pub parent_reply_count: i64,
}

impl ThreadReplyContext {
    pub async fn load(
        state: &AppState,
        parent_id: i64,
        message: &Message,
    ) -> Result<Arc<Self>, AppError> {
        let base = MessageRenderBase::load(state, message).await?;
        let prior = db::chat::thread_reply_prior(&state.chat, parent_id, message.id).await?;
        let parent_reply_count = db::chat::count_replies(&state.chat, parent_id).await?;
        Ok(Arc::new(Self {
            base,
            prior,
            parent_reply_count,
        }))
    }
}

/// A per-viewer lookup that already degraded to a default before LC-780. The
/// degrade stays (the message still renders), but the failure is logged with
/// the ids that locate it instead of vanishing into the default.
pub(crate) fn warn_default<T: Default, E: std::fmt::Display>(
    result: Result<T, E>,
    what: &str,
    message_id: i64,
    viewer_id: &str,
) -> T {
    result.unwrap_or_else(|e| {
        tracing::warn!(error = %e, what, message_id, viewer_id, "per-viewer render lookup failed; rendering with its default");
        T::default()
    })
}
