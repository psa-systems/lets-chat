// Shared template context. Currently just a marker module; templates use
// per-page structs. The base template parameters are duplicated by every page
// struct that extends it.

/// One row in the sidebar's "Rooms" section. Carries the unread count so the
/// included `partials/unread_badge.html` can render a badge per room. Carries
/// `mentions` so `partials/mention_badge.html` can render an `@N` chip when
/// the viewer has unread mentions in this room.
pub struct SidebarRoom {
    pub id: i64,
    pub name: String,
    pub unread: i64,
    pub mentions: i64,
    /// "none" | "except_mentions" | "all". Carried as a string (not a Rust
    /// enum) because Askama templates compare against literals.
    pub mute_mode: String,
    /// True for voice channels. Lets the sidebar pick the channel icon and
    /// skip the unread/mention badges (voice channels have no messages).
    pub is_voice: bool,
    pub active: bool,
}

/// One row in the sidebar's "Direct messages" section. Carries the unread
/// count so the included `partials/unread_badge.html` can render a badge per
/// peer.
pub struct SidebarPeer {
    pub id: String,
    /// LC-80: chat.db `rooms.id` of the DM. Stars key on room ids
    /// (so both regular rooms and DMs share `starred_rooms`); the
    /// sidebar loader buckets DMs into the Starred section and the
    /// star-toggle form targets this id.
    pub dm_room_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    pub unread: i64,
    pub status: String,
    pub custom_status: Option<String>,
    /// "none" | "all". DMs only ever take these two; mirrors
    /// `SidebarRoom::mute_mode` so the sidebar template applies the same
    /// greyed-link treatment uniformly.
    pub mute_mode: String,
    pub active: bool,
}

impl SidebarPeer {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }
}

/// A user-defined sidebar category plus the rooms the user has assigned
/// to it. The sidebar partial iterates these groups first (in
/// `position` order) before falling back to the flat `sidebar_rooms`
/// list, which now holds only rooms the user has NOT categorized.
/// `collapsed` is a per-user toggle persisted in `auth.db`; the template
/// hides the room list when true and shows the category header alone.
pub struct SidebarCategoryGroup {
    pub id: i64,
    /// The enclave this category lives in. Surfaced so the sidebar
    /// template can build the enclave-scoped admin URLs
    /// (`/enclave/{enclave_id}/sidebar/categories/...`) without
    /// threading current_enclave through every include scope.
    pub enclave_id: i64,
    pub name: String,
    pub collapsed: bool,
    pub rooms: Vec<SidebarRoom>,
    /// Sum of `room.unread` across rooms whose `mute_mode == "none"`.
    /// Mirrors the per-room unread badge's mute behaviour: muted rooms
    /// suppress their own badge, so they should not bump the
    /// collapsed-category aggregate either.
    pub unread_total: i64,
    /// Sum of `room.mentions` across all rooms in the category.
    /// Mentions ignore the per-room mute mode (the per-room mention
    /// badge does too) so the aggregate is the raw sum.
    pub mention_total: i64,
}

/// One icon in the leftmost enclave-switcher column. `id = None` is the Home
/// pseudo-enclave (DM hub). `unread` aggregates unread counts across all
/// rooms (or DMs, for Home) the user can see in that scope. `pending_invites`
/// is set on the Home entry only and is added to the displayed badge.
pub struct SwitcherEntry {
    pub id: Option<i64>,
    pub label: String,
    pub initial: String,
    pub unread: i64,
    pub pending_invites: i64,
    pub active: bool,
    /// LC-141: branding logo URL for this scope, when one is set. The Home
    /// entry resolves the global logo (`/branding/logo`); enclave entries
    /// resolve `/enclave/{id}/branding/logo` (which itself falls back to the
    /// global logo). `None` when no logo exists for this scope, in which case
    /// the template renders the single-letter `initial` instead.
    pub logo_url: Option<String>,
    /// LC-143: true when the viewer may manage this enclave (owner/admin, or
    /// site admin). Drives the settings gear on the active enclave's tile.
    /// Always false for the Home entry.
    pub can_manage: bool,
}
