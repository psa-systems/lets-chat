//! LC-298: user profile hovercard fragment.
//!
//! A small popover shown when hovering a username/avatar (delegated controller
//! in `layout.html`). Reuses `partials/avatar.html` for the avatar + live
//! status dot. Bio is gated by the handler (`show_bio`); the Message action is
//! gated by `can_message` so it never links to a DM the recipient's privacy /
//! block settings would reject.
use crate::i18n::filters;
use askama::Template;

#[derive(Template)]
#[template(path = "partials/hovercard.html")]
pub struct HovercardFragment {
    pub user_id: String,
    pub username: String,
    /// display_name when set and non-empty, else username.
    pub label: String,
    pub avatar_ext: Option<String>,
    /// Effective (presence-resolved) status: active/idle/away/dnd/offline.
    pub status: String,
    pub custom_status: Option<String>,
    pub is_bot: bool,
    /// Bio, already privacy-gated by the handler (None = do not show).
    pub bio: Option<String>,
    /// Render the "Message" link to /dm/{user_id}.
    pub can_message: bool,
}
