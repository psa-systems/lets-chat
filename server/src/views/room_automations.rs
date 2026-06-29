//! LC-495: workflow-automations section on the room manage page. The same
//! partial (`partials/room_automations.html`) is `{% include %}`d by the manage
//! page and rendered standalone by the create/toggle/delete handlers, so a
//! mutation swaps `#lc-automations` in place without a full reload (the shared-
//! partial live-update convention).

#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // in scope for the |t filter

use crate::db::automations::RoomAutomation;
use crate::models::Room;

/// One rule, prepared for display.
pub struct AutomationRow {
    pub id: i64,
    pub enabled: bool,
    pub name: Option<String>,
    pub trigger_kind: String,
    /// `None` when the rule fires on any occurrence (empty filter).
    pub match_text: Option<String>,
    /// Short preview of the action body for the list row.
    pub action_preview: String,
}

/// Max chars of the action body shown in the list preview.
const PREVIEW_CHARS: usize = 80;

impl AutomationRow {
    pub fn from_rule(r: RoomAutomation) -> Self {
        let match_text = r
            .match_text
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let mut action_preview: String = r.action_body.chars().take(PREVIEW_CHARS).collect();
        if r.action_body.chars().count() > PREVIEW_CHARS {
            action_preview.push('\u{2026}'); // ellipsis
        }
        AutomationRow {
            id: r.id,
            enabled: r.enabled,
            name: r.name.filter(|s| !s.trim().is_empty()),
            trigger_kind: r.trigger_kind,
            match_text,
            action_preview,
        }
    }
}

/// Standalone render of the automations section (the create/toggle/delete
/// handlers return this; the manage page includes the same partial).
#[derive(Template)]
#[template(path = "partials/room_automations.html")]
pub struct RoomAutomationsFragment<'a> {
    pub room: &'a Room,
    pub automations: &'a [AutomationRow],
}
