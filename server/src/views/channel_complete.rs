//! LC-323: composer `#channel` autocomplete popover + the render-time room
//! reference. Mirrors `views::emoji_complete` / `views::mentions`: a small
//! `<ul role="listbox">` the composer swaps into `#lc-channel-popover`, each
//! row inserting a `#name` token. `ChannelRef` also rides on `MessageView` so
//! `render_body` can rewrite `#name` -> a link to `/room/{id}` at render time.
#[allow(unused_imports)]
use crate::i18n::filters; // in-scope for the `|t` filter in channel_popover.html.
use askama::Template;

/// A linkable room reference within an enclave. `name` is the room name (always
/// a linkable charset - see `routes::is_linkable_channel_name`); `room_id` is
/// the link target. Carried on `MessageView` and used by `render_body`.
#[derive(Clone)]
pub struct ChannelRef {
    pub name: String,
    pub room_id: i64,
}

#[derive(Template)]
#[template(path = "partials/channel_popover.html")]
pub struct ChannelPopoverFragment<'a> {
    pub results: &'a [ChannelRef],
}
