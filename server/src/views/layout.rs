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
    pub active: bool,
}

/// One row in the sidebar's "Direct messages" section. Carries the unread
/// count so the included `partials/unread_badge.html` can render a badge per
/// peer.
pub struct SidebarPeer {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
    pub unread: i64,
    pub status: String,
    pub custom_status: Option<String>,
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
}
