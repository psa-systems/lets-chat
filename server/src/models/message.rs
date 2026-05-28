use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub room_id: i64,
    pub user_id: String,
    pub author_name: String,
    pub body: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    /// `Some(N)` when this message is a reply in the thread rooted at `N`.
    pub parent_id: Option<i64>,
    /// `Some(N)` when this message visually quotes the message with id `N`
    /// inline above its body. Distinct from `parent_id`: quoted messages
    /// still appear in the main timeline rather than being collapsed into
    /// a thread side panel. Always `None` for thread replies (the quote-
    /// reply affordance is suppressed inside the thread panel).
    pub quote_id: Option<i64>,
    /// True for server-authored system notices (e.g. "started a call"),
    /// which render as a centered, non-interactive line.
    pub is_system: bool,
    /// LC-74: `Some(N)` when posted by incoming webhook `N` (user_id is empty).
    pub webhook_id: Option<i64>,
    /// LC-77: `Some(N)` when posted by email-ingress inbox `N` (user_id is
    /// empty). Parallel to `webhook_id`; exactly one of the two is `Some`
    /// for any synthetic-actor message, both `None` for real-user messages.
    pub email_inbox_id: Option<i64>,
    /// LC-78: `Some(N)` when posted by protocol bridge `N` (user_id is
    /// empty). At most one of webhook_id / email_inbox_id / bridge_id is
    /// `Some` for any given message.
    pub bridge_id: Option<i64>,
    /// LC-78: snapshotted foreign display name (e.g. Matrix `alice:server`).
    /// `Some` iff `bridge_id` is `Some`. Carried on the Message broadcast so
    /// the WS render reaches the actor resolver without a re-query; same
    /// pass-through shape as `webhook_id` + the join-resolved webhook name.
    pub bridge_foreign_name: Option<String>,
    /// LC-78: snapshotted protocol kind (`matrix` / `irc` / `xmpp`). `Some`
    /// iff `bridge_id` is `Some`.
    pub bridge_kind: Option<String>,
    /// LC-78-AVATAR-PROXY: the cache key for the message's foreign avatar.
    /// `Some` when the daemon submitted a foreign avatar URL and the proxy
    /// gate was enabled at submit time. Carried on the broadcast Message so
    /// the WS render reaches the resolver without a re-query, matching the
    /// pass-through shape of `bridge_foreign_name` and `bridge_kind`.
    pub bridge_foreign_avatar: Option<String>,
}
