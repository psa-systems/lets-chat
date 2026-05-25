//! Actor identity for a message-render view.
//!
//! Messages can be authored by a real user (LC-1+), an incoming webhook
//! (LC-74), or an email-ingress inbox (LC-77).
//!
//! The User variant is unit because real-user identity scalars (id,
//! username, avatar_ext, status, custom_status, is_bot) already live on
//! MessageView and are shared across actor types. The Webhook and
//! EmailInbox variants each carry only the actor-shape-specific override
//! (the configured avatar URL); the synthetic actor's display name is
//! populated into MessageView::username by routes::resolve_msg_author at
//! construction time, so the template can read it the same way for all
//! three actors.
//!
//! Tuple variants chosen to match the only Askama match precedent in
//! this codebase (`{% when Some with (d) %}`); struct-like variants
//! would need different Askama syntax that has no in-repo precedent.

#[derive(Debug, Clone)]
pub enum MessageActor {
    User,
    /// Incoming webhook (LC-74). The tuple field carries the webhook's
    /// configured avatar URL (None = render initials).
    Webhook(Option<String>),
    /// LC-77: email-ingress inbox. Mail addressed to the inbox's
    /// secret address is posted to its room as this actor. Same
    /// "secret IS the authorization" trust model as Webhook; same
    /// avatar-or-initials render shape.
    EmailInbox(Option<String>),
}
