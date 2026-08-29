//! LC-834: the `page_context` frame, which is what keeps a WebSocket's
//! page-scoped state honest once one socket serves more than one page.
//!
//! Three pieces of `ws.rs` state describe the PAGE rather than the connection -
//! `current_enclave`, `subscribed` and `dm_seen_msg` - and all three are correct
//! today only because every navigation is a full page load and therefore a fresh
//! socket. The frame's contract is fresh-socket equivalence: after it is
//! applied, the connection holds exactly what a socket opened from scratch on
//! the destination page would hold.
//!
//! The sharpest of the three is `current_enclave`, because the frame is the only
//! thing that can ever CLEAR it. It is learned from an `enclave:{id}` topic
//! subscription, and Home and DM pages have always sent no topic at all, so
//! silence cannot also mean "left the enclave". The first test below is that
//! assertion end to end: one connection, enclave A, then the no-enclave frame,
//! and `render_sidebar` output that changes shape between the two.
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
    /// An enclave the user is a member of, and a room inside it.
    enclave_id: i64,
    enclave_room: i64,
    /// A second room in the same enclave, for the navigation-between-rooms case.
    other_room: i64,
    /// An enclave the user is NOT a member of.
    foreign_enclave: i64,
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

    // create_enclave adds the creator as a member, so the user is in the first
    // and a stranger to the second.
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
    let stranger = db::auth::create_user(&auth, "stranger", "hash")
        .await
        .unwrap();
    let foreign_enclave = db::enclave::create_enclave(&chat, "Not Yours", None, &stranger)
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
        enclave_id,
        enclave_room,
        other_room,
        foreign_enclave,
    }
}

/// The three page-scoped bindings `handle_socket` owns, so a test can drive the
/// frame exactly as the receive loop does.
#[derive(Default)]
struct PageState {
    scope: PageScope,
}

impl PageState {
    /// Returns what the receive loop acts on: whether the frame moved the
    /// connection to a different page (LC-836).
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

    fn enclave(&self) -> Option<i64> {
        *self.scope.current_enclave.lock().unwrap()
    }

    fn room(&self) -> Option<i64> {
        self.scope.page.lock().unwrap().and_then(|p| p.room_id)
    }

    fn rooms(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self
            .scope
            .subscribed
            .lock()
            .unwrap()
            .iter()
            .copied()
            .collect();
        v.sort_unstable();
        v
    }
}

// ---------------------------------------------------------------------------
// The acceptance criterion: a topic change mid-connection changes what the
// server renders for that connection.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_no_enclave_frame_mid_connection_changes_the_sidebar_shape() {
    let fx = fixture().await;
    let (conn_id, _rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let page = PageState::default();

    // Land on a room inside enclave A, exactly as that page's markup reports it.
    page.navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
        .await;
    assert_eq!(page.enclave(), Some(fx.enclave_id));
    let in_enclave = render_sidebar(&fx.state, &fx.user, page.enclave(), page.room())
        .await
        .expect("the sidebar renders for an enclave page");
    let enclave_nav = format!("id=\"sidebar-nav-{}\"", fx.enclave_id);
    assert!(
        in_enclave.contains(&enclave_nav),
        "an enclave page's sidebar carries the enclave-keyed nav id, got:\n{in_enclave}"
    );

    // Navigate to Home on the SAME connection. Home sends no enclave topic, so
    // only an explicit null can distinguish "left the enclave" from "this page
    // never had one" - which is the whole reason the frame exists.
    page.navigate(&fx, conn_id, None, None).await;
    assert_eq!(
        page.enclave(),
        None,
        "the no-enclave frame must clear the connection's enclave"
    );
    let at_home = render_sidebar(&fx.state, &fx.user, page.enclave(), page.room())
        .await
        .expect("the sidebar renders for Home");
    assert!(
        !at_home.contains(&enclave_nav),
        "Home's sidebar must not still be keyed to the enclave just left, got:\n{at_home}"
    );
    assert!(
        at_home.contains("id=\"sidebar-nav\""),
        "Home's sidebar carries the unkeyed nav id, got:\n{at_home}"
    );
    assert_ne!(
        in_enclave, at_home,
        "the two renders must differ; if they do not, the frame changed nothing"
    );

    // And back again, so the change is a replacement rather than a one-way latch.
    page.navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
        .await;
    assert_eq!(page.enclave(), Some(fx.enclave_id));
    assert!(
        render_sidebar(&fx.state, &fx.user, page.enclave(), page.room())
            .await
            .expect("the sidebar renders again")
            .contains(&enclave_nav)
    );
}

#[tokio::test]
async fn the_enclave_topic_subscription_follows_the_frame() {
    let fx = fixture().await;
    let (conn_id, mut rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let page = PageState::default();
    let topic = format!("enclave:{}", fx.enclave_id);

    page.navigate(&fx, conn_id, None, Some(fx.enclave_id)).await;
    fx.hub.broadcast_to_topic(
        &topic,
        &ChatEvent::EnclaveMemberAdded {
            enclave_id: fx.enclave_id,
            user_id: fx.user.id.clone(),
        },
    );
    assert!(
        matches!(rx.try_recv(), Ok(ChatEvent::EnclaveMemberAdded { .. })),
        "a connection on an enclave page receives that enclave's topic events"
    );

    // Leaving for Home must leave the hub's fan-out set too, not just the local
    // copy: otherwise the connection keeps receiving an enclave's updates from
    // a page that is no longer in it.
    page.navigate(&fx, conn_id, None, None).await;
    fx.hub.broadcast_to_topic(
        &topic,
        &ChatEvent::EnclaveMemberAdded {
            enclave_id: fx.enclave_id,
            user_id: fx.user.id.clone(),
        },
    );
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "after the no-enclave frame the connection must be out of the enclave topic"
    );
}

// ---------------------------------------------------------------------------
// `subscribed`: audited as MUST DROP. A retained room is not inert - the
// foreground branch of render_new_message_or_bump advances the read watermark
// and broadcasts a DmRead receipt for it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_room_left_behind_is_dropped_from_the_subscription_set() {
    let fx = fixture().await;
    let (conn_id, mut rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let page = PageState::default();

    page.navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
        .await;
    assert_eq!(page.rooms(), vec![fx.enclave_room]);

    page.navigate(&fx, conn_id, Some(fx.other_room), Some(fx.enclave_id))
        .await;
    assert_eq!(
        page.rooms(),
        vec![fx.other_room],
        "a connection must hold the destination room only, never accumulate"
    );

    // The hub's mirror of the set follows, so the old room's events stop
    // arriving rather than being rendered into the new room's message list.
    fx.hub.broadcast_to_room(
        fx.enclave_room,
        &ChatEvent::UserStoppedTyping {
            room_id: fx.enclave_room,
            user_id: fx.user.id.clone(),
        },
    );
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "the room the user navigated away from must no longer fan out to them"
    );
    fx.hub.broadcast_to_room(
        fx.other_room,
        &ChatEvent::UserStoppedTyping {
            room_id: fx.other_room,
            user_id: fx.user.id.clone(),
        },
    );
    assert!(
        matches!(rx.try_recv(), Ok(ChatEvent::UserStoppedTyping { .. })),
        "the destination room does fan out"
    );

    // Home holds no room at all.
    page.navigate(&fx, conn_id, None, None).await;
    assert!(page.rooms().is_empty());
}

// ---------------------------------------------------------------------------
// `dm_seen_msg`: audited as MUST CLEAR on a page move (a fresh socket's caption
// map is empty), and MUST NOT be touched when the page did not change.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_seen_caption_memory_is_cleared_by_a_move_and_kept_by_a_re_assert() {
    let fx = fixture().await;
    let (conn_id, _rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let page = PageState::default();

    page.navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
        .await;
    page.scope
        .dm_seen_msg
        .lock()
        .unwrap()
        .insert(fx.enclave_room, 42);

    // Re-asserting the SAME page is what LC-318's reconnect soft-refresh does on
    // a live socket today. It must change nothing at all.
    page.navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
        .await;
    assert_eq!(
        page.scope.dm_seen_msg.lock().unwrap().get(&fx.enclave_room),
        Some(&42),
        "a same-page re-assert must not disturb the caption memory"
    );

    // An actual move must leave the map as a fresh socket would find it: empty.
    // Keeping it would leave the connection clearing a caption slot in a DOM
    // that no longer holds it, and double-captioning the room on return.
    page.navigate(&fx, conn_id, Some(fx.other_room), Some(fx.enclave_id))
        .await;
    assert!(
        page.scope.dm_seen_msg.lock().unwrap().is_empty(),
        "a page move must clear the caption memory"
    );

    // An enclave-only move (an enclave landing page holds no room) counts too.
    page.scope
        .dm_seen_msg
        .lock()
        .unwrap()
        .insert(fx.other_room, 7);
    page.navigate(&fx, conn_id, Some(fx.other_room), None).await;
    assert!(
        page.scope.dm_seen_msg.lock().unwrap().is_empty(),
        "leaving the enclave is a page move even when the room is unchanged"
    );
}

// ---------------------------------------------------------------------------
// Authorization parity: the replacement frame may never grant what the additive
// `subscribe` / `subscribe_topic` frames would refuse.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refused_destination_is_treated_as_absent_not_ignored() {
    let fx = fixture().await;
    let (conn_id, mut rx, _) = fx.hub.connect(&fx.user.id, "navigator");
    let page = PageState::default();

    page.navigate(&fx, conn_id, Some(fx.enclave_room), Some(fx.enclave_id))
        .await;

    // A crafted frame naming an enclave the viewer is not in. The claim is
    // dropped, and - the part that matters - the page they were on is still torn
    // down, so a refusal cannot be used to pin a connection to a stale enclave.
    page.navigate(&fx, conn_id, None, Some(fx.foreign_enclave))
        .await;
    assert_eq!(
        page.enclave(),
        None,
        "an unauthorized enclave must not be adopted"
    );
    assert!(
        page.rooms().is_empty(),
        "the previous page's room is dropped even when the new claim is refused"
    );
    fx.hub.broadcast_to_topic(
        &format!("enclave:{}", fx.foreign_enclave),
        &ChatEvent::EnclaveMemberAdded {
            enclave_id: fx.foreign_enclave,
            user_id: fx.user.id.clone(),
        },
    );
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "a refused enclave never joins the hub's fan-out set"
    );
}
