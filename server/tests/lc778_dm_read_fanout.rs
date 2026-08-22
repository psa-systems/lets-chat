//! LC-778: a foreground viewer's read-advance no longer rebroadcasts room-wide.
//!
//! `render_new_message_or_bump` advances the watermark of every viewer holding
//! the room open and announces it with a `DmRead`. That announcement used to go
//! to the whole room, so one posted message in a room with M foreground viewers
//! produced M broadcasts times M recipients: M squared `render_dm_read` runs,
//! each up to five queries. The event is now addressed to the only two parties
//! that render anything from it - the message author and the reader's own other
//! tabs - so the cost is linear in M.
//!
//! These tests pin that contract by counting delivered `DmRead` events, which is
//! exactly the number of `render_dm_read` invocations the WS send task performs.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use lets_chat::models::Message;
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, routes};
use tokio::sync::broadcast::Receiver;

mod common;

/// Drain whatever is queued on a receiver without blocking.
fn drain(rx: &mut Receiver<ChatEvent>) -> Vec<ChatEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

/// The `(reader, last_read_message_id)` pairs of the `DmRead` events delivered
/// to one connection.
fn dm_reads(events: &[ChatEvent]) -> Vec<(String, i64)> {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::DmRead {
                user_id,
                last_read_message_id,
                ..
            } => Some((user_id.clone(), *last_read_message_id)),
            _ => None,
        })
        .collect()
}

struct Fixture {
    state: lets_chat::state::AppState,
    room: i64,
    author: String,
    viewers: Vec<String>,
    message: Message,
}

/// One room of `room_type` holding an author plus `viewer_count` other members,
/// with a single message already posted by the author.
async fn setup(room_type: &str, viewer_count: usize) -> Fixture {
    let dir = std::env::temp_dir().join(format!("lc-778-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let author = db::auth::create_user(&auth, "author", "h").await.unwrap();
    let mut viewers = Vec::new();
    for i in 0..viewer_count {
        viewers.push(
            db::auth::create_user(&auth, &format!("viewer{i}"), "h")
                .await
                .unwrap(),
        );
    }
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let room = db::chat::create_room(&chat, "lc778", None, room_type, None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, room, &author)
        .await
        .unwrap();
    for v in &viewers {
        db::chat::add_room_member(&chat, room, v).await.unwrap();
    }

    let msg_id = db::chat::insert_message(&chat, room, &author, "hello")
        .await
        .unwrap();
    let raw = db::chat::get_message(&chat, msg_id).await.unwrap().unwrap();
    let message = Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: raw.user_id,
        author_name: "author".to_string(),
        body: raw.body,
        created_at: raw.created_at,
        edited_at: raw.edited_at,
        parent_id: raw.parent_id,
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: raw.webhook_id,
        email_inbox_id: raw.email_inbox_id,
        bridge_id: raw.bridge_id,
        bridge_foreign_name: raw.bridge_foreign_name,
        bridge_kind: raw.bridge_kind,
        bridge_foreign_avatar: raw.bridge_foreign_avatar,
    };

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = lets_chat::state::AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
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

    Fixture {
        state,
        room,
        author,
        viewers,
        message,
    }
}

/// The real row, so the tests exercise what the WS send task passes.
async fn user(state: &lets_chat::state::AppState, id: &str) -> lets_chat::models::User {
    db::auth::find_user_by_id(&state.auth, id)
        .await
        .unwrap()
        .expect("user exists")
        .into()
}

/// Deliver the new message to `viewer` with the room open in the foreground -
/// the branch that advances the watermark and announces the read.
async fn deliver_foreground(
    state: &lets_chat::state::AppState,
    message: &Message,
    viewer: &lets_chat::models::User,
    room: i64,
) -> Option<String> {
    let subscribed = Arc::new(Mutex::new(HashSet::from([room])));
    routes::render_new_message_or_bump(state, message, None, viewer, &subscribed).await
}

/// One message read by N foreground viewers costs 2N `DmRead` deliveries (each
/// viewer tells the author and its own other tabs), never N * (N + 1) - what the
/// room-wide broadcast cost when every connection in the room was subscribed.
#[tokio::test]
async fn dm_read_fanout_is_linear_in_foreground_viewers() {
    for n in [2usize, 4, 8] {
        let f = setup("private", n).await;

        // Every participant is connected AND subscribed to the room, so a
        // room-wide broadcast would reach all of them. That is what makes this
        // a regression guard rather than a restatement of the new code.
        let (author_conn, mut author_rx, _) = f.state.hub.connect(&f.author, "author");
        f.state.hub.subscribe(author_conn, f.room);
        let mut viewer_rx: Vec<Receiver<ChatEvent>> = Vec::new();
        for v in &f.viewers {
            let (conn, rx, _) = f.state.hub.connect(v, "viewer");
            f.state.hub.subscribe(conn, f.room);
            viewer_rx.push(rx);
        }

        for v in &f.viewers {
            let viewer = user(&f.state, v).await;
            deliver_foreground(&f.state, &f.message, &viewer, f.room).await;
        }

        let author_reads = dm_reads(&drain(&mut author_rx));
        assert_eq!(
            author_reads.len(),
            n,
            "n={n}: the author is told once per reader"
        );
        assert!(
            author_reads.iter().all(|(_, id)| *id == f.message.id),
            "n={n}: every read announces the posted message"
        );

        let mut total = author_reads.len();
        for (i, rx) in viewer_rx.iter_mut().enumerate() {
            let reads = dm_reads(&drain(rx));
            assert_eq!(
                reads,
                vec![(f.viewers[i].clone(), f.message.id)],
                "n={n}: viewer {i} sees only its own read, never another viewer's"
            );
            total += reads.len();
        }
        assert_eq!(
            total,
            2 * n,
            "n={n}: total DmRead deliveries must scale as 2n, not n * (n + 1)"
        );
    }
}

/// A DM peer reading in the foreground still reaches the author's connections,
/// which is what renders the "Seen" caption.
#[tokio::test]
async fn dm_read_still_reaches_the_dm_author() {
    let f = setup("dm", 1).await;
    let peer = f.viewers[0].clone();

    let (_ac, mut author_rx, _) = f.state.hub.connect(&f.author, "author");
    let (_pc, mut peer_rx, _) = f.state.hub.connect(&peer, "peer");

    let viewer = user(&f.state, &peer).await;
    deliver_foreground(&f.state, &f.message, &viewer, f.room).await;

    assert_eq!(
        dm_reads(&drain(&mut author_rx)),
        vec![(peer.clone(), f.message.id)],
        "the author must be told the peer read, or the Seen caption never renders"
    );
    assert_eq!(
        dm_reads(&drain(&mut peer_rx)),
        vec![(peer, f.message.id)],
        "the reader's other tabs still clear their badge"
    );
}

/// The group "Seen by" bar still ships with the message render for an eligible
/// viewer, and a viewer with receipts off still gets none (the LC-778 skip of
/// the room lookup for that viewer must not change what they see).
#[tokio::test]
async fn group_seen_bar_still_renders_with_the_message() {
    let f = setup("private", 2).await;
    let bar_id = format!("id=\"lc-seen-{}\"", f.room);

    let viewer = user(&f.state, &f.viewers[0]).await;
    let html = deliver_foreground(&f.state, &f.message, &viewer, f.room)
        .await
        .expect("foreground viewer gets the message render");
    assert!(
        html.contains(&bar_id),
        "an eligible viewer still gets the Seen by bar with the message"
    );

    sqlx::query("UPDATE users SET read_receipts_enabled = 0 WHERE id = ?")
        .bind(&f.viewers[1])
        .execute(&f.state.auth)
        .await
        .unwrap();
    let off = user(&f.state, &f.viewers[1]).await;
    let html = deliver_foreground(&f.state, &f.message, &off, f.room)
        .await
        .expect("foreground viewer gets the message render");
    assert!(
        !html.contains(&bar_id),
        "a viewer with read receipts off renders no Seen by bar"
    );
}
