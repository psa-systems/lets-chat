//! LC-342: the per-message shame-tag control (the "Flag" popover), rendered
//! lazily into the message hover menu and re-rendered after a vote / override.

#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template;

/// One tag row in the control: its name, current in-window vote count, and
/// whether the viewer has voted it.
pub struct ShameTagRow {
    pub tag: String,
    pub count: i64,
    pub voted: bool,
}

#[derive(Template)]
#[template(path = "room/shame_tag_control.html")]
pub struct ShameTagControl {
    pub message_id: i64,
    pub tags: Vec<ShameTagRow>,
    /// Viewer can moderate (force show/hide). Shows the override controls.
    pub can_manage: bool,
    /// Current override: Some(true) force-hidden, Some(false) force-shown, None.
    pub override_hidden: Option<bool>,
}
