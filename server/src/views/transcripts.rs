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

/// LC-394: one row on the transcripts archive page.
pub struct TranscriptIndexRow {
    pub id: i64,
    /// Headline: the DM peer's name, or the voice channel's name.
    pub title: String,
    /// Sub-line: "Direct call" or "Voice channel".
    pub kind_label: String,
    /// LC-443: true for a DM (Direct call) transcript, false for a voice
    /// channel - drives the badge colour without re-deriving from kind_label.
    pub is_dm: bool,
    pub started_at: String,
    pub ended: bool,
    pub segment_count: i64,
    /// LC-441: the viewer may delete this one (started it, or is a site admin).
    pub can_delete: bool,
}

/// LC-394: the transcripts archive (every transcript the viewer can access).
#[derive(Template)]
#[template(path = "transcripts/index.html")]
pub struct TranscriptIndexPage<'a> {
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
    pub rows: Vec<TranscriptIndexRow>,
    /// LC-395: the current search query (echoed into the search box).
    pub query: String,
    /// LC-442: whether the hide-empty (0-line) filter is on.
    pub hide_empty: bool,
}

/// LC-445: just the list body (rows or empty state), returned for htmx search /
/// filter / bulk-delete swaps into `#lc-transcript-list`.
#[derive(Template)]
#[template(path = "transcripts/list_body.html")]
pub struct TranscriptListBody {
    pub rows: Vec<TranscriptIndexRow>,
    pub query: String,
    pub hide_empty: bool,
}

/// LC-441: a standalone out-of-band toast (reuses the global #lc-toast-region +
/// toast.js). Appended to a delete response so the action confirms success/error.
#[derive(Template)]
#[template(path = "transcripts/toast_oob.html")]
pub struct ToastOob {
    pub ok: bool,
    pub message: String,
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
    /// LC-396: true when an LLM endpoint is configured (shows the Summarize UI).
    pub llm_available: bool,
    /// LC-396: the rendered (markdown -> HTML) summary, if one exists.
    pub summary_html: Option<String>,
}

/// LC-396: the htmx response after generating a summary - the rendered summary
/// + a regenerate button, swapped into `#lc-transcript-summary`.
#[derive(Template)]
#[template(path = "transcripts/summary_fragment.html")]
pub struct TranscriptSummaryFragment {
    pub transcript_id: i64,
    pub summary_html: String,
}
