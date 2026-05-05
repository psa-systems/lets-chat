// Shared template context. Currently just a marker module; templates use
// per-page structs. The base template parameters are duplicated by every page
// struct that extends it.

/// One row in the sidebar's "Rooms" section. Carries the unread count so the
/// included `partials/unread_badge.html` can render a badge per room.
pub struct SidebarRoom {
    pub id: i64,
    pub name: String,
    pub unread: i64,
}

/// One row in the sidebar's "Direct messages" section. Carries the unread
/// count so the included `partials/unread_badge.html` can render a badge per
/// peer.
pub struct SidebarPeer {
    pub id: String,
    pub username: String,
    pub unread: i64,
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
