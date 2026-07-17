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
    /// LC-576: the header's favorite star; recomputed here so it does not reset
    /// to unstarred when the notify-prefs POST swaps the whole header.
    pub is_starred: bool,
    /// LC-84: gates the "Moderators" link in the header. Same value the
    /// `RoomPage` view computed; recomputed here on the notify-prefs
    /// swap path so the link does not vanish when the user toggles mute.
    pub can_manage_overrides: bool,
    /// LC-484: gates the "Catch me up" action; recomputed here so it
    /// survives the notify-prefs header swap.
    pub llm_available: bool,
    /// LC-506: admin-only disabled teaser when the LLM is unconfigured.
    pub llm_teaser: bool,
    /// LC-553: header member avatar stack, recomputed here so the stack survives
    /// the notify-prefs header swap (mirrors RoomPage).
    pub member_count: usize,
    pub header_members: Vec<String>,
}
