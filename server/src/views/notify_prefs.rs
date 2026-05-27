#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::Room;

/// The room header swap target (`#lc-room-header`). Returned by the
/// notify-prefs POST so the requesting tab updates inline.
#[derive(Template)]
#[template(path = "partials/room_header.html")]
pub struct RoomHeaderFragment<'a> {
    pub room: &'a Room,
    pub mute_mode: &'a str,
    /// LC-84: gates the "Moderators" link in the header. Same value the
    /// `RoomPage` view computed; recomputed here on the notify-prefs
    /// swap path so the link does not vanish when the user toggles mute.
    pub can_manage_overrides: bool,
}
