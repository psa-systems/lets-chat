//! LC-494: stage control-plane view (speakers vs listeners + request-to-speak).
//! Roster + per-viewer controls only; audio transport is the LC-512 SFU
//! follow-up, so the panel carries an explicit "audio coming soon" note.

#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t filter.

/// One person on the stage roster.
pub struct StageMember {
    pub user_id: String,
    pub label: String,
    /// Listener who has requested the floor (always false for speakers).
    pub hand_raised: bool,
}

#[derive(Template)]
#[template(path = "partials/stage_panel.html")]
pub struct StagePanel {
    pub room_id: i64,
    pub speakers: Vec<StageMember>,
    /// Listeners (non-speakers); `hand_raised` flags those requesting the floor.
    pub listeners: Vec<StageMember>,
    /// The viewer can grant/revoke the floor (enclave owner/admin or room
    /// moderator). Drives the approve / remove controls.
    pub is_host: bool,
    /// The viewer is currently on the stage.
    pub is_participant: bool,
    /// The viewer holds the floor.
    pub is_speaker: bool,
    /// The viewer has a raised hand.
    pub hand_raised: bool,
    /// True when this fragment is an OOB live swap (vs the initial page render).
    pub oob: bool,
}

impl StagePanel {
    /// Number of people on the stage (speakers + listeners), for the heading.
    pub fn total(&self) -> usize {
        self.speakers.len() + self.listeners.len()
    }
}
