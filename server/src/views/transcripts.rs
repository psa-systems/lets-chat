#[allow(unused_imports)]
use crate::i18n::filters; // LC-188: in-scope for the |t/|tn template filters.
use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

/// One line of a saved transcript, speaker pre-resolved by the handler.
pub struct TranscriptLine {
    pub speaker_name: String,
    pub text: String,
    pub spoken_at: String,
}

/// LC-393: the saved call-transcript page. Carries the standard sidebar chrome
/// (via `routes::load_chrome`) plus the transcript body.
#[derive(Template)]
#[template(path = "transcripts/show.html")]
pub struct TranscriptPage<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub transcript_id: i64,
    pub room_name: String,
    pub started_at: String,
    pub ended: bool,
    pub lines: Vec<TranscriptLine>,
}
