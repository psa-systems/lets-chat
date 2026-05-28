//! Actor identity for a message-render view.
//!
//! Messages can be authored by a real user (LC-1+), an incoming webhook
//! (LC-74), an email-ingress inbox (LC-77), or a protocol bridge (LC-78).
//!
//! The User variant is unit because real-user identity scalars (id,
//! username, avatar_ext, status, custom_status, is_bot) already live on
//! MessageView and are shared across actor types. The Webhook, EmailInbox,
//! and Bridge variants each carry only the actor-shape-specific overrides;
//! the synthetic actor's display name is populated into MessageView::username
//! by routes::resolve_msg_author at construction time, so the template can
//! read it the same way for all actors.
//!
//! Tuple variants chosen to match the only Askama match precedent in
//! this codebase (`{% when Some with (d) %}`); struct-like variants
//! would need different Askama syntax that has no in-repo precedent. The
//! Bridge variant wraps a struct INSIDE the tuple so it can carry both
//! kind (for "via matrix" labeling) and avatar_url under the same shape.

#[derive(Debug, Clone)]
pub struct BridgeActorMeta {
    /// Protocol kind snapshotted from bridges.kind at post time and
    /// stored on messages.bridge_kind. Snapshotted (not joined) so the
    /// "(via <kind>)" label survives bridge-row removal under stop-new
    /// semantics. v1 only ever sees 'matrix'.
    pub kind: String,
    /// Always None in v1: the bridge-messages endpoint 400s any non-null
    /// avatar_url to avoid leaking viewer IPs to arbitrary federated
    /// homeservers on per-render fetch. The LC-78-AVATAR-PROXY follow-up
    /// will populate this with a proxy-cached URL.
    pub avatar_url: Option<String>,
}

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
    /// LC-78: out-of-process protocol bridge (Matrix in v1; IRC / XMPP
    /// are pure daemon work, no schema change). The bridge daemon POSTs
    /// each foreign-protocol message to the bridge-messages endpoint
    /// with a per-message foreign_name + (future) foreign_avatar that
    /// are SNAPSHOTTED onto the message row. Unlike Webhook / EmailInbox,
    /// the actor identity does NOT join back to a fixed channel row at
    /// render time, because the set of foreign actors is open-ended.
    Bridge(BridgeActorMeta),
}
