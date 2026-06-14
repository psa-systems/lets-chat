// LC-278: message forwarding. The picker (swapped into the singleton
// `#lc-forward-modal`) lists the viewer's post-able rooms + DMs; choosing one
// POSTs the forward and the confirm fragment replaces the picker in the slot.
#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template;

/// One forward destination row (a room or a DM).
pub struct ForwardDest {
    pub room_id: i64,
    /// Leading glyph: `#` for a room, `@` for a DM.
    pub glyph: String,
    /// Display label (`room name` / DM peer label).
    pub label: String,
    /// Lowercased searchable tokens for the client-side filter.
    pub name: String,
}

#[derive(Template)]
#[template(path = "forward/picker.html")]
pub struct ForwardPicker {
    pub message_id: i64,
    pub rooms: Vec<ForwardDest>,
    pub dms: Vec<ForwardDest>,
}

/// Inline confirmation rendered into the modal slot after a successful forward.
#[derive(Template)]
#[template(path = "forward/confirm.html")]
pub struct ForwardConfirm {
    pub dest_label: String,
}
