//! LC-66 integration: polls / voting.
//!
//! Covers atomic creation (modal + slash command), single vs multi voting,
//! the closed-poll 409, anonymous voter privacy, and message-delete cascade.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-polls-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    alice: String,
    alice_session: String,
    bob: String,
    chat: SqlitePool,
    auth: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    for id in [&alice, &bob] {
        sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
            .bind(id)
            .execute(&auth)
            .await
            .unwrap();
    }
    let alice_session = db::auth::create_session(&auth, &alice).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
        chat: chat.clone(),
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
    TestApp {
        app: routes::build_router(state),
        alice,
        alice_session,
        bob,
        chat,
        auth,
    }
}

async fn send(
    app: &Router,
    sess: Option<&str>,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(s) = sess {
        b = b.header(header::COOKIE, format!("session={s}"));
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Public room in an enclave Alice owns.
async fn seed_room(t: &TestApp) -> i64 {
    let eid = db::enclave::create_enclave(&t.chat, "Acme", None, &t.alice)
        .await
        .unwrap();
    db::chat::create_room(&t.chat, "general", None, "public", None, Some(eid))
        .await
        .unwrap()
}

async fn latest_message_id(chat: &SqlitePool, room_id: i64) -> i64 {
    sqlx::query_scalar("SELECT id FROM messages WHERE room_id = ? ORDER BY id DESC LIMIT 1")
        .bind(room_id)
        .fetch_one(chat)
        .await
        .unwrap()
}

async fn option_ids(chat: &SqlitePool, message_id: i64) -> Vec<i64> {
    sqlx::query("SELECT id FROM poll_options WHERE message_id = ? ORDER BY position")
        .bind(message_id)
        .fetch_all(chat)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.get::<i64, _>("id"))
        .collect()
}

#[tokio::test]
async fn create_poll_is_atomic() {
    let t = app().await;
    let room = seed_room(&t).await;
    let (status, _) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/room/{room}/poll"),
        "question=Lunch%3F&options=Pizza%0ATacos%0ASushi",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mid = latest_message_id(&t.chat, room).await;
    // messages row + polls row + 3 options, all present.
    let poll = db::polls::get(&t.chat, mid).await.unwrap();
    assert!(poll.is_some(), "polls row created");
    assert_eq!(option_ids(&t.chat, mid).await.len(), 3, "three options");
    let body: String = sqlx::query_scalar("SELECT body FROM messages WHERE id = ?")
        .bind(mid)
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(body, "Lunch?", "question stored as the message body");
}

#[tokio::test]
async fn create_poll_clamps_huge_closes_in_without_panicking() {
    // LC-350: a huge closes_in must not overflow chrono's Duration math and
    // panic the handler (500). It is clamped to the 1-year max and the poll is
    // created with a bounded close time.
    let t = app().await;
    let room = seed_room(&t).await;
    let (status, body) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/room/{room}/poll"),
        "question=Q%3F&options=A%0AB&closes_in=9999999999999999",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "should clamp, not 500: {body}");
    let mid = latest_message_id(&t.chat, room).await;
    let poll = db::polls::get(&t.chat, mid).await.unwrap();
    assert!(poll.is_some(), "poll created with a clamped close time");
}

#[tokio::test]
async fn single_choice_moves_and_toggles() {
    let t = app().await;
    let room = seed_room(&t).await;
    let mid = db::polls::create(
        &t.chat,
        room,
        &t.alice,
        "Pick one",
        &["A".into(), "B".into()],
        false,
        false,
        None,
    )
    .await
    .unwrap();
    let opts = option_ids(&t.chat, mid).await;

    // Vote A.
    send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/poll/{mid}/vote"),
        &format!("option_id={}", opts[0]),
    )
    .await;
    assert_eq!(
        db::polls::user_votes(&t.chat, mid, &t.alice).await.unwrap(),
        vec![opts[0]]
    );

    // Vote B -> moves (single choice).
    send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/poll/{mid}/vote"),
        &format!("option_id={}", opts[1]),
    )
    .await;
    assert_eq!(
        db::polls::user_votes(&t.chat, mid, &t.alice).await.unwrap(),
        vec![opts[1]]
    );

    // Vote B again -> toggles off.
    send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/poll/{mid}/vote"),
        &format!("option_id={}", opts[1]),
    )
    .await;
    assert!(db::polls::user_votes(&t.chat, mid, &t.alice)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn multi_choice_allows_multiple() {
    let t = app().await;
    let room = seed_room(&t).await;
    let mid = db::polls::create(
        &t.chat,
        room,
        &t.alice,
        "Pick many",
        &["A".into(), "B".into(), "C".into()],
        true,
        false,
        None,
    )
    .await
    .unwrap();
    let opts = option_ids(&t.chat, mid).await;
    for &o in &[opts[0], opts[2]] {
        send(
            &t.app,
            Some(&t.alice_session),
            Method::POST,
            &format!("/poll/{mid}/vote"),
            &format!("option_id={o}"),
        )
        .await;
    }
    let mut mine = db::polls::user_votes(&t.chat, mid, &t.alice).await.unwrap();
    mine.sort();
    let mut want = vec![opts[0], opts[2]];
    want.sort();
    assert_eq!(mine, want, "both choices recorded");
}

#[tokio::test]
async fn closed_poll_rejects_vote() {
    let t = app().await;
    let room = seed_room(&t).await;
    let mid = db::polls::create(
        &t.chat,
        room,
        &t.alice,
        "Old poll",
        &["A".into(), "B".into()],
        false,
        false,
        Some("2000-01-01 00:00:00"),
    )
    .await
    .unwrap();
    let opts = option_ids(&t.chat, mid).await;
    let (status, _) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/poll/{mid}/vote"),
        &format!("option_id={}", opts[0]),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "voting on a closed poll is 409"
    );
}

#[tokio::test]
async fn anonymous_poll_hides_voters() {
    let t = app().await;
    let room = seed_room(&t).await;
    let mid = db::polls::create(
        &t.chat,
        room,
        &t.alice,
        "Secret ballot",
        &["A".into(), "B".into()],
        false,
        true, // anonymous
        None,
    )
    .await
    .unwrap();
    let opts = option_ids(&t.chat, mid).await;
    db::polls::add_vote(&t.chat, opts[0], &t.alice)
        .await
        .unwrap();

    let view = lets_chat::views::room::build_poll_view(&t.chat, &t.auth, mid, &t.bob)
        .await
        .unwrap()
        .unwrap();
    assert!(view.anonymous);
    assert_eq!(view.options[0].count, 1, "count still shown");
    assert!(
        view.options.iter().all(|o| o.voters.is_empty()),
        "anonymous poll never exposes voter identities"
    );
}

#[tokio::test]
async fn deleting_message_cascades_poll() {
    let t = app().await;
    let room = seed_room(&t).await;
    let mid = db::polls::create(
        &t.chat,
        room,
        &t.alice,
        "Doomed",
        &["A".into(), "B".into()],
        false,
        false,
        None,
    )
    .await
    .unwrap();
    let opts = option_ids(&t.chat, mid).await;
    db::polls::add_vote(&t.chat, opts[0], &t.alice)
        .await
        .unwrap();

    // Hard delete the anchor message; FK cascade clears poll/options/votes.
    sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(mid)
        .execute(&t.chat)
        .await
        .unwrap();

    assert!(
        db::polls::get(&t.chat, mid).await.unwrap().is_none(),
        "poll gone"
    );
    let opt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM poll_options WHERE message_id = ?")
            .bind(mid)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(opt_count, 0, "options gone");
    let vote_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poll_votes WHERE option_id = ?")
        .bind(opts[0])
        .fetch_one(&t.chat)
        .await
        .unwrap();
    assert_eq!(vote_count, 0, "votes gone");
}

#[tokio::test]
async fn slash_command_posts_poll() {
    let t = app().await;
    let room = seed_room(&t).await;
    let (status, _) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/room/{room}/messages"),
        // body = /poll "Q" "A" "B"  (url-encoded)
        "body=%2Fpoll%20%22Fav%3F%22%20%22A%22%20%22B%22&file_id=&quote_id=",
    )
    .await;
    // LC-228: slash dispatch returns 204 (form `hx-swap="none"`).
    assert_eq!(status, StatusCode::NO_CONTENT);
    let mid = latest_message_id(&t.chat, room).await;
    let poll = db::polls::get(&t.chat, mid).await.unwrap();
    assert!(poll.is_some(), "slash command created a poll");
    assert_eq!(option_ids(&t.chat, mid).await.len(), 2);
}

#[tokio::test]
async fn non_member_cannot_vote() {
    let t = app().await;
    // Private room in Alice's enclave; Bob is not an enclave member.
    let eid = db::enclave::create_enclave(&t.chat, "Acme", None, &t.alice)
        .await
        .unwrap();
    let room = db::chat::create_room(&t.chat, "secret", None, "private", Some("code"), Some(eid))
        .await
        .unwrap();
    db::chat::add_room_member(&t.chat, room, &t.alice)
        .await
        .unwrap();
    let mid = db::polls::create(
        &t.chat,
        room,
        &t.alice,
        "members only",
        &["A".into(), "B".into()],
        false,
        false,
        None,
    )
    .await
    .unwrap();
    let opts = option_ids(&t.chat, mid).await;
    let bob_session = db::auth::create_session(&t.auth, &t.bob).await.unwrap();
    let (status, _) = send(
        &t.app,
        Some(&bob_session),
        Method::POST,
        &format!("/poll/{mid}/vote"),
        &format!("option_id={}", opts[0]),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "non-member cannot vote");
    assert_eq!(
        db::polls::voter_count(&t.chat, mid).await.unwrap(),
        0,
        "no vote recorded"
    );
}

// LC-491: events are polls with event_at + fixed RSVP options; RSVP reuses the
// vote path and the event surfaces in the iCal source query.
#[tokio::test]
async fn create_event_makes_poll_with_event_at_and_rsvp() {
    let t = app().await;
    let room = seed_room(&t).await;
    let (status, body) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/room/{room}/event"),
        "title=Launch+party&event_at=2030-01-02T18%3A30%3A00Z&location=HQ",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mid = latest_message_id(&t.chat, room).await;
    let poll = db::polls::get(&t.chat, mid)
        .await
        .unwrap()
        .expect("poll row");
    assert_eq!(poll.event_at.as_deref(), Some("2030-01-02 18:30:00"));
    assert_eq!(poll.event_location.as_deref(), Some("HQ"));
    let opts = option_ids(&t.chat, mid).await;
    assert_eq!(opts.len(), 3, "Going / Maybe / Can't go");

    // RSVP reuses the poll vote path.
    let (vst, _) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/poll/{mid}/vote"),
        &format!("option_id={}", opts[0]),
    )
    .await;
    assert_eq!(vst, StatusCode::OK);
    assert_eq!(
        db::polls::user_votes(&t.chat, mid, &t.alice).await.unwrap(),
        vec![opts[0]]
    );

    // Surfaces in the iCal source query.
    let events = db::polls::events_for_room(&t.chat, room).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, "Launch party");
    assert_eq!(events[0].2, "2030-01-02 18:30:00");
}

#[tokio::test]
async fn create_event_rejects_invalid_time() {
    let t = app().await;
    let room = seed_room(&t).await;
    let (status, _) = send(
        &t.app,
        Some(&t.alice_session),
        Method::POST,
        &format!("/room/{room}/event"),
        "title=X&event_at=not-a-date",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
