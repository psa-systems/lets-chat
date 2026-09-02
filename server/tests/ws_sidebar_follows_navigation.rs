//! LC-836: the sidebar stays correct without a page load.
//!
//! `layout.html` renders the sidebar outside `<main id="main">`, so a
//! navigation that swaps only `#main` (LC-837) leaves it showing the page the
//! tab came from: the wrong active row, and an unread badge the page GET just
//! cleared. The fix rides the existing whole-sidebar OOB path (`render_sidebar`,
//! the one mark-all-read and mute already use) and is driven server-side from
//! the LC-834 `page_context` frame, which already tells the server where the
//! client went: `apply_page_context` reports a real move, the receive loop
//! sends `ChatEvent::PageChanged` to that ONE connection, and the send task
//! re-renders its sidebar for the destination.
//!
//! Two things the tests pin that are easy to lose: the connect frame and a
//! same-url re-assert (LC-318's reconnect soft-refresh) are NOT moves, so a
//! full page load today gets no redundant OOB swap of a sidebar it just
//! rendered; and the refresh reaches only the connection that moved, never the
//! user's other tabs.
use lets_chat::models::User;
use lets_chat::push::{MockPushClient, PushClient};
use lets_chat::routes::test_support::{apply_page_context, render_sidebar, PageContext, PageScope};
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, state::AppState};
use std::sync::Arc;
use tokio::sync::broadcast::error::TryRecvError;

mod common;

struct Fx {
    state: AppState,
    hub: Arc<Hub>,
    user: User,
    /// A second user: the DM peer, and the author of the unread messages.
    peer_id: String,
    enclave_id: i64,
    enclave_room: i64,
    other_room: i64,
}

async fn fixture() -> Fx {
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let user_id = db::auth::create_user(&auth, "navigator", "hash")
        .await
        .unwrap();
    let user: User = db::auth::find_user_by_id(&auth, &user_id)
        .await
        .unwrap()
        .unwrap()
        .into();
    let peer_id = db::auth::create_user(&auth, "peer", "hash").await.unwrap();

    let enclave_id = db::enclave::create_enclave(&chat, "Team", None, &user_id)
        .await
        .unwrap();
    let enclave_room =
        db::chat::create_room(&chat, "general", None, "public", None, Some(enclave_id))
            .await
            .unwrap();
    let other_room = db::chat::create_room(&chat, "random", None, "public", None, Some(enclave_id))
        .await
        .unwrap();

    let hub = Arc::new(Hub::new());
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub: hub.clone(),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(MockPushClient::default()) as Arc<dyn PushClient>,
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
        bunyip_sso: None,
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };

    Fx {
        state,
        hub,
        user,
        peer_id,
        enclave_id,
        enclave_room,
        other_room,
    }
}

/// The page-scoped bindings `handle_socket` owns, driven exactly as the receive
/// loop drives them.
#[derive(Default)]
struct PageState {
    scope: PageScope,
}

impl PageState {
    /// Returns whether the frame moved the connection, which is what decides
    /// the `PageChanged` push.
    async fn navigate(
        &self,
        fx: &Fx,
        conn_id: u64,
        room_id: Option<i64>,
        enclave_id: Option<i64>,
    ) -> bool {
        apply_page_context(
            &fx.state,
            &fx.user,
            conn_id,
            &self.scope,
            PageContext {
                room_id,
                enclave_id,
            },
        )
        .await
    }

    /// The sidebar exactly as the send task renders it for this connection.
    async fn sidebar(&self, fx: &Fx) -> String {
        let enclave = *self.scope.current_enclave.lock().unwrap();
        let room = self.scope.page.lock().unwrap().and_then(|p| p.room_id);
        render_sidebar(&fx.state, &fx.user, enclave, room)
            .await
            .expect("the sidebar renders")
    }
}

/// Whether the anchor whose `href` is exactly `href` carries `aria-current="page"`
/// somewhere in its opening tag. Scoped to the tag rather than an adjacency
/// check, because other attributes (LC-837's per-anchor boost) sit between
/// `href` and `aria-current`.
fn anchor_is_active(html: &str, href: &str) -> bool {
    let needle = format!("href=\"{href}\"");
    let mut from = 0;
    while let Some(i) = html[from..].find(&needle) {
        let start = from + i;
        let end = html[start..]
            .find('>')
            .map(|e| start + e)
            .unwrap_or(html.len());
        if html[start..end].contains("aria-current=\"page\"") {
            return true;
        }
        from = end;
    }
    false
}

fn room_active(html: &str, id: i64) -> bool {
    anchor_is_active(html, &format!("/room/{id}"))
}

fn peer_active(html: &str, id: &str) -> bool {
    anchor_is_active(html, &format!("/dm/{id}"))
}

// ---------------------------------------------------------------------------
// Which frames are moves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_connect_frame_and_a_same_page_reassert_are_not_moves() {
    let fx = fixture().await;
    let (conn_id, _rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let page = PageState::default();

    // The connect frame: the page load just rendered this sidebar itself, so a
    // refresh would only re-swap `#sidebar` (and reset its scroll) for nothing.
    assert!(
        !page
            .navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
            .await,
        "the first frame on a connection is not a move"
    );
    // LC-318's reconnect soft-refresh re-asserts the same url.
    assert!(
        !page
            .navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
            .await,
        "a same-page re-assert is not a move"
    );
    // A different room in the same enclave is.
    assert!(
        page.navigate(&fx, conn_id, Some(fx.other_room), Some(fx.enclave_id))
            .await,
        "a different room is a move"
    );
    // And so is leaving for Home, where only the enclave changes.
    assert!(
        page.navigate(&fx, conn_id, None, None).await,
        "leaving the enclave is a move"
    );
    assert!(
        !page.navigate(&fx, conn_id, None, None).await,
        "re-asserting Home is not a move"
    );
}

// ---------------------------------------------------------------------------
// What the refresh renders.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_oob_sidebar_highlights_the_room_the_connection_is_on() {
    let fx = fixture().await;
    let (conn_id, _rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let page = PageState::default();

    page.navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
        .await;
    let first = page.sidebar(&fx).await;
    assert!(
        room_active(&first, fx.enclave_room),
        "the room the connection is on carries aria-current, got:\n{first}"
    );
    assert!(
        !room_active(&first, fx.other_room),
        "no other room is active, got:\n{first}"
    );
    // LC-865: the enclave rail (outside #sidebar) follows the move too - it
    // lights the enclave the connection is in, not Home.
    assert!(
        anchor_is_active(&first, &format!("/enclave/{}", fx.enclave_id)),
        "the rail lights the current enclave, got:\n{first}"
    );
    assert!(
        !anchor_is_active(&first, "/?home=1"),
        "the rail does not light Home while in an enclave, got:\n{first}"
    );

    // Move rooms on the same connection: the highlight follows.
    page.navigate(&fx, conn_id, Some(fx.other_room), Some(fx.enclave_id))
        .await;
    let second = page.sidebar(&fx).await;
    assert!(
        room_active(&second, fx.other_room),
        "the destination room is active after the move, got:\n{second}"
    );
    assert!(
        !room_active(&second, fx.enclave_room),
        "the room just left is no longer active, got:\n{second}"
    );

    // A page with no room (Home) highlights no sidebar row. Scope the check to
    // the #sidebar fragment: render_sidebar now also emits the enclave rail OOB
    // (LC-865), whose Home tile is legitimately lit here.
    page.navigate(&fx, conn_id, None, None).await;
    let home = page.sidebar(&fx).await;
    let home_sidebar = home.split("<nav id=\"switcher\"").next().unwrap_or(&home);
    assert!(
        !home_sidebar.contains("aria-current=\"page\""),
        "Home highlights no sidebar row, got:\n{home}"
    );
    // LC-865: and the rail follows back to Home - it lights the Home tile and
    // drops the enclave it just left.
    assert!(
        anchor_is_active(&home, "/?home=1"),
        "the rail lights Home, got:\n{home}"
    );
    assert!(
        !anchor_is_active(&home, &format!("/enclave/{}", fx.enclave_id)),
        "the rail drops the enclave just left, got:\n{home}"
    );
}

#[tokio::test]
async fn a_dm_page_highlights_its_peer_row_by_the_dm_room() {
    let fx = fixture().await;
    let dm = db::chat::create_dm_room(&fx.state.chat, "dm", &fx.user.id, &fx.peer_id)
        .await
        .unwrap();
    let (conn_id, _rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let page = PageState::default();

    // The DM page reports its chat.db room and, being outside every enclave,
    // an explicit null enclave. The peer row is keyed by the peer's user id in
    // the markup, so the mapping room -> peer is what this pins.
    page.navigate(&fx, conn_id, Some(dm.id), None).await;
    let html = page.sidebar(&fx).await;
    assert!(
        peer_active(&html, &fx.peer_id),
        "the DM's peer row carries aria-current, got:\n{html}"
    );
}

#[tokio::test]
async fn the_unread_badge_of_the_opened_room_is_cleared_by_the_refresh() {
    let fx = fixture().await;
    let (conn_id, _rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let page = PageState::default();

    // Three unread messages from someone else in the room about to be opened.
    let mut last = 0;
    for body in ["one", "two", "three"] {
        last = db::chat::insert_message(&fx.state.chat, fx.other_room, &fx.peer_id, body)
            .await
            .unwrap();
    }
    page.navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
        .await;
    let before = page.sidebar(&fx).await;
    let badge_id = format!("id=\"unread-room-{}\"", fx.other_room);
    assert!(
        before.contains(&format!(
            "{badge_id} data-lc-unread class=\"lc-count-pill\">3<"
        )),
        "the room not yet opened shows its unread count, got:\n{before}"
    );

    // Opening the room: the page GET marks it read before the swap lands, then
    // the frame reports the move and the refresh renders the cleared badge.
    db::chat::set_last_read(&fx.state.chat, &fx.user.id, fx.other_room, last)
        .await
        .unwrap();
    assert!(
        page.navigate(&fx, conn_id, Some(fx.other_room), Some(fx.enclave_id))
            .await
    );
    let after = page.sidebar(&fx).await;
    assert!(
        after.contains(&format!("{badge_id} data-lc-unread></span>")),
        "the opened room's badge is empty after the refresh, got:\n{after}"
    );
    assert!(
        room_active(&after, fx.other_room),
        "and it is the active row, got:\n{after}"
    );
}

// ---------------------------------------------------------------------------
// Who the refresh reaches.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_refresh_reaches_only_the_connection_that_moved() {
    let fx = fixture().await;
    // Two tabs of the same user.
    let (moving, mut moving_rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let (_other, mut other_rx, _) = fx.hub.connect(&fx.user.id, "navigator");

    fx.hub.send_to_conn(
        moving,
        &ChatEvent::PageChanged {
            user_id: fx.user.id.clone(),
        },
    );

    assert!(
        matches!(
            moving_rx.try_recv(),
            Ok(ChatEvent::PageChanged { ref user_id }) if user_id == &fx.user.id
        ),
        "the tab that moved is told to re-render its sidebar"
    );
    assert!(
        matches!(other_rx.try_recv(), Err(TryRecvError::Empty)),
        "the user's other tab, which did not move, receives nothing"
    );
}
