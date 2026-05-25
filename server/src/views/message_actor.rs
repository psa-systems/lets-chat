//! Actor identity for a message-render view.
//!
//! Messages can be authored by a real user (LC-1+) or an incoming webhook
//! (LC-74). LC-77 adds per-room email ingress as a third actor case; that
//! variant lands in the commit that introduces the email_inboxes schema.
//!
//! The User variant is unit because real-user identity scalars (id,
//! username, avatar_ext, status, custom_status, is_bot) already live on
//! MessageView and are shared across actor types. The Webhook variant
//! carries only the actor-shape-specific override (the webhook's
//! configured avatar URL); the webhook's display name is populated into
//! MessageView::username by routes::resolve_msg_author at construction
//! time, so the template can read it the same way for both actors.

pub enum MessageActor {
    User,
    /// Incoming webhook (LC-74). The tuple field carries the webhook's
    /// configured avatar URL (None = render initials). Tuple variant
    /// chosen to match the only Askama match precedent in this codebase
    /// (`{% when Some with (d) %}`); struct-like variants would need
    /// different Askama syntax that has no in-repo precedent.
    Webhook(Option<String>),
}

impl MessageActor {
    /// Construct from the (is_webhook, avatar_url) shape that
    /// `routes::AuthorMeta` exposes. Used by every MessageView
    /// construction site so the if/else stays in one place.
    pub fn from_webhook_flag(is_webhook: bool, webhook_avatar_url: Option<String>) -> Self {
        if is_webhook {
            Self::Webhook(webhook_avatar_url)
        } else {
            Self::User
        }
    }
}
