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
