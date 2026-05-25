//! LC-77-REPLY: Askama view structs for the per-message mention + DM
//! notification email. Two parallel templates (text + HTML) render the
//! same logical content from one set of fields; the dispatcher
//! constructs the same struct shape for both renderers.

use askama::Template;

#[derive(Template)]
#[template(path = "email/notification.html")]
pub struct NotificationHtml<'a> {
    pub sender_label: &'a str,
    pub room_label: &'a str,
    /// Pre-rendered deep link to the message in chat. The dispatcher
    /// constructs this from `state.base_url` + `/room/{room_id}#msg-{id}`.
    pub message_url: &'a str,
    pub snippet_html: &'a str,
    /// True when the event is a DM (kind = "dm"); false for an @mention
    /// in a room. The template branches on this for the subject + intro line.
    pub is_dm: bool,
    /// True when the operator has the email-ingress domain configured; the
    /// Reply-To affordance text is gated on this.
    pub reply_supported: bool,
    pub reply_expires_hours: i64,
    pub settings_url: &'a str,
}

#[derive(Template)]
#[template(path = "email/notification.txt")]
pub struct NotificationText<'a> {
    pub sender_label: &'a str,
    pub room_label: &'a str,
    pub message_url: &'a str,
    pub snippet_plain: &'a str,
    pub is_dm: bool,
    pub reply_supported: bool,
    pub reply_expires_hours: i64,
    pub settings_url: &'a str,
}
