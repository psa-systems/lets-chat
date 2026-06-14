//! Integration tests for `routes::drafts` (LC-64 server-persisted drafts).
//!
//! Covers the four handler paths (visibility 403 / empty-delete /
//! race-guard noop / upsert), the 60-day lazy cleanup (asserting the
//! stale row is actually deleted, not just absent from the render),
//! the three clear hooks (send / schedule / purge), the cascade via
//! the FK on `room_id`, the privacy invariant (user A cannot write
//! to user B's slot), and the race guard's 5-second window (both
//! in-window-noop and past-window-upserts).
//!
//! Also pins the EXPLAIN QUERY PLAN of the race-guard SELECT so a
//! future schema refactor that drops an index this query depends on
//! surfaces in test rather than as production latency.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::{Row, SqlitePool};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-drafts-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    alice_session: String,
    bob_session: String,
    alice_id: String,
    bob_id: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&chat)
        .await
        .unwrap();
    let alice_id = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let bob_id = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&alice_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&bob_id)
        .execute(&auth)
        .await
        .unwrap();
    let alice_session = db::auth::create_session(&auth, &alice_id).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg: bg.clone(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        alice_session,
        bob_session,
        alice_id,
        bob_id,
        chat: chat_for_test,
    }
}

async fn send(
    app: &Router,
    sess: &str,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes).to_string();
    (status, body_str)
}

async fn private_room(chat: &SqlitePool, name: &str) -> i64 {
    sqlx::query("INSERT INTO rooms (name, room_type) VALUES (?, 'private')")
        .bind(name)
        .execute(chat)
        .await
        .unwrap()
        .last_insert_rowid()
}

// ----------------------------------------------------------------------
// Four handler paths.
// ----------------------------------------------------------------------

#[tokio::test]
async fn put_creates_draft_row_with_204() {
    let t = app().await;
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=hello+world",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let stored = db::drafts::get(&t.chat, &t.alice_id, 1).await.unwrap();
    assert_eq!(
        stored.as_ref().map(|r| r.body.as_str()),
        Some("hello world")
    );
}

#[tokio::test]
async fn put_updates_existing_row_lww() {
    let t = app().await;
    send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=first",
    )
    .await;
    let first = db::drafts::get(&t.chat, &t.alice_id, 1)
        .await
        .unwrap()
        .unwrap();
    // SQLite datetime('now') has 1-second resolution; sleep just past it.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=second",
    )
    .await;
    let second = db::drafts::get(&t.chat, &t.alice_id, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.body, "second");
    assert!(
        second.updated_at > first.updated_at,
        "second PUT must advance updated_at (LWW): first={} second={}",
        first.updated_at,
        second.updated_at,
    );
}

#[tokio::test]
async fn put_empty_body_deletes_row() {
    let t = app().await;
    send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=hello",
    )
    .await;
    assert!(db::drafts::get(&t.chat, &t.alice_id, 1)
        .await
        .unwrap()
        .is_some());

    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(db::drafts::get(&t.chat, &t.alice_id, 1)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn put_whitespace_only_body_deletes_row() {
    // Trim happens BEFORE the empty check, so whitespace-only bodies
    // hit the delete branch, not an upsert with "   " stored.
    let t = app().await;
    send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=hello",
    )
    .await;
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=+++",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(db::drafts::get(&t.chat, &t.alice_id, 1)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn put_to_private_room_non_member_returns_403() {
    let t = app().await;
    let room_id = private_room(&t.chat, "secret").await;
    // Alice is admin so she'd pass the access check. Bob is a plain user
    // who is not a member of the private room.
    let (status, _) = send(
        &t.app,
        &t.bob_session,
        Method::PUT,
        &format!("/room/{room_id}/draft"),
        "body=intruder",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(db::drafts::get(&t.chat, &t.bob_id, room_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn put_writes_to_authenticated_user_slot_not_other() {
    // Privacy invariant: the PK is (user_id, room_id) and user_id comes
    // from the session, not the form body. Alice's PUT to room 1
    // cannot touch Bob's draft in room 1, and vice versa.
    let t = app().await;
    send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=alice+text",
    )
    .await;
    send(
        &t.app,
        &t.bob_session,
        Method::PUT,
        "/room/1/draft",
        "body=bob+text",
    )
    .await;

    let alice_draft = db::drafts::get(&t.chat, &t.alice_id, 1)
        .await
        .unwrap()
        .unwrap();
    let bob_draft = db::drafts::get(&t.chat, &t.bob_id, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alice_draft.body, "alice text");
    assert_eq!(bob_draft.body, "bob text");
}

// ----------------------------------------------------------------------
// Race guard. The PUT handler 204-no-ops when the trimmed body
// matches a `messages` row from this user in this room created within
// the last 5 seconds. Catches the send-then-resurrect race; trim-
// aligns against the send path's already-trimmed storage shape.
// ----------------------------------------------------------------------

async fn insert_message_at(
    chat: &SqlitePool,
    room_id: i64,
    user_id: &str,
    body: &str,
    age_seconds: i64,
) -> i64 {
    let id: i64 = sqlx::query("INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?)")
        .bind(room_id)
        .bind(user_id)
        .bind(body)
        .execute(chat)
        .await
        .unwrap()
        .last_insert_rowid();
    if age_seconds != 0 {
        sqlx::query("UPDATE messages SET created_at = datetime('now', ?) WHERE id = ?")
            .bind(format!("-{age_seconds} seconds"))
            .bind(id)
            .execute(chat)
            .await
            .unwrap();
    }
    id
}

#[tokio::test]
async fn race_guard_blocks_within_5s_window() {
    let t = app().await;
    // A message Alice just "sent" in room 1 with body "hi".
    insert_message_at(&t.chat, 1, &t.alice_id, "hi", 1).await;
    // A trailing draft PUT for the same body lands inside the 5s window.
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=hi",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        db::drafts::get(&t.chat, &t.alice_id, 1)
            .await
            .unwrap()
            .is_none(),
        "race guard must refuse the upsert (no resurrect)",
    );
}

#[tokio::test]
async fn race_guard_allows_after_5s_window() {
    let t = app().await;
    // The same shape, but the message is 10 seconds old; the guard's
    // window is 5s, so the PUT proceeds normally.
    insert_message_at(&t.chat, 1, &t.alice_id, "hi", 10).await;
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        "body=hi",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let draft = db::drafts::get(&t.chat, &t.alice_id, 1).await.unwrap();
    assert_eq!(draft.map(|r| r.body), Some("hi".to_string()));
}

#[tokio::test]
async fn race_guard_trim_aligns_against_stored_body() {
    // The send path trims before storing (post_message at line 397):
    // form.body.trim() -> messages.body. The draft body on the racing
    // PUT may carry trailing whitespace ("hi\n" if the Enter keyup
    // included a newline); the handler trims the draft before the
    // race-guard SELECT so the trimmed forms match.
    //
    // This test pins the trim alignment: stored message is "hi" (already
    // trimmed); the PUT sends "hi\n" which trims to "hi"; the guard hits
    // and the PUT no-ops.
    let t = app().await;
    insert_message_at(&t.chat, 1, &t.alice_id, "hi", 1).await;
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/1/draft",
        // urlencoded "hi\n"
        "body=hi%0A",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        db::drafts::get(&t.chat, &t.alice_id, 1)
            .await
            .unwrap()
            .is_none(),
        "trim-aligned race guard must still block; otherwise the silent-resurrect bug ships",
    );
}

#[tokio::test]
async fn race_guard_does_not_block_different_user() {
    // Alice's recent message must not block Bob's draft. The race
    // guard is user-scoped (WHERE user_id = ? on the SELECT).
    let t = app().await;
    insert_message_at(&t.chat, 1, &t.alice_id, "hi", 1).await;
    let (status, _) = send(
        &t.app,
        &t.bob_session,
        Method::PUT,
        "/room/1/draft",
        "body=hi",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let draft = db::drafts::get(&t.chat, &t.bob_id, 1).await.unwrap();
    assert_eq!(draft.map(|r| r.body), Some("hi".to_string()));
}

#[tokio::test]
async fn race_guard_does_not_block_different_room() {
    // Alice's recent message in room 1 must not block her draft in
    // room 2. The race guard is room-scoped too.
    let t = app().await;
    insert_message_at(&t.chat, 1, &t.alice_id, "hi", 1).await;
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::PUT,
        "/room/2/draft",
        "body=hi",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let draft = db::drafts::get(&t.chat, &t.alice_id, 2).await.unwrap();
    assert_eq!(draft.map(|r| r.body), Some("hi".to_string()));
}

// ----------------------------------------------------------------------
// Lazy cleanup on load. The 60-day rule must actually DELETE the row,
// not just decline to return it. This pins the get_fresh_or_purge
// semantic: stale-or-purge, not stale-or-keep-but-hide.
// ----------------------------------------------------------------------

#[tokio::test]
async fn stale_draft_is_purged_on_load_not_just_hidden() {
    let t = app().await;
    db::drafts::upsert(&t.chat, &t.alice_id, 1, "old text")
        .await
        .unwrap();
    // Backdate to 61 days ago. The 60-day threshold lives in the
    // get_room handler; the helper takes max_age_days as a parameter
    // so the test exercises the helper directly here for clarity.
    sqlx::query("UPDATE message_drafts SET updated_at = datetime('now', '-61 days') WHERE user_id = ? AND room_id = ?")
        .bind(&t.alice_id)
        .bind(1i64)
        .execute(&t.chat)
        .await
        .unwrap();

    let result = db::drafts::get_fresh_or_purge(&t.chat, &t.alice_id, 1, 60)
        .await
        .unwrap();
    assert!(result.is_none(), "stale draft must not be returned");

    // The load-bearing assertion: the row is actually GONE from the
    // table, not just filtered out of this call's response. If the
    // helper merely declines to return without deleting, drafts
    // accumulate forever (never-cleanup, not lazy-cleanup).
    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM message_drafts WHERE user_id = ? AND room_id = ?")
            .bind(&t.alice_id)
            .bind(1i64)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(
        row_count, 0,
        "stale draft must be DELETED by get_fresh_or_purge"
    );
}

#[tokio::test]
async fn fresh_draft_returned_unchanged_by_get_fresh_or_purge() {
    let t = app().await;
    db::drafts::upsert(&t.chat, &t.alice_id, 1, "fresh")
        .await
        .unwrap();
    let result = db::drafts::get_fresh_or_purge(&t.chat, &t.alice_id, 1, 60)
        .await
        .unwrap();
    assert_eq!(result, Some("fresh".to_string()));
    // Row still present.
    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM message_drafts WHERE user_id = ? AND room_id = ?")
            .bind(&t.alice_id)
            .bind(1i64)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(row_count, 1);
}

// ----------------------------------------------------------------------
// Three clear hooks: send / schedule / purge.
// ----------------------------------------------------------------------

#[tokio::test]
async fn send_clears_draft_via_finalize_message_send() {
    let t = app().await;
    db::drafts::upsert(&t.chat, &t.alice_id, 1, "draft text")
        .await
        .unwrap();
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/room/1/messages",
        "body=actual+message",
    )
    .await;
    assert!(
        status.is_success() || status.is_redirection(),
        "send must succeed, got {status}",
    );
    assert!(
        db::drafts::get(&t.chat, &t.alice_id, 1)
            .await
            .unwrap()
            .is_none(),
        "send (finalize_message_send) must clear the draft",
    );
}

#[tokio::test]
async fn account_delete_purges_drafts_across_rooms() {
    let t = app().await;
    db::drafts::upsert(&t.chat, &t.alice_id, 1, "in room 1")
        .await
        .unwrap();
    db::drafts::upsert(&t.chat, &t.alice_id, 2, "in room 2")
        .await
        .unwrap();
    db::drafts::upsert(&t.chat, &t.bob_id, 1, "bob in room 1")
        .await
        .unwrap();

    // delete_for_user is the function purge_user_chat calls inside its
    // transaction. Exercising it directly against the chat pool here.
    let mut tx = t.chat.begin().await.unwrap();
    db::drafts::delete_for_user(&mut *tx, &t.alice_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(db::drafts::get(&t.chat, &t.alice_id, 1)
        .await
        .unwrap()
        .is_none());
    assert!(db::drafts::get(&t.chat, &t.alice_id, 2)
        .await
        .unwrap()
        .is_none());
    // Bob's draft must survive.
    assert!(db::drafts::get(&t.chat, &t.bob_id, 1)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn room_delete_cascades_drafts_via_fk() {
    let t = app().await;
    let temp_room = private_room(&t.chat, "ephemeral").await;
    db::drafts::upsert(&t.chat, &t.alice_id, temp_room, "in ephemeral")
        .await
        .unwrap();
    db::drafts::upsert(&t.chat, &t.alice_id, 1, "in general")
        .await
        .unwrap();

    sqlx::query("DELETE FROM rooms WHERE id = ?")
        .bind(temp_room)
        .execute(&t.chat)
        .await
        .unwrap();

    assert!(
        db::drafts::get(&t.chat, &t.alice_id, temp_room)
            .await
            .unwrap()
            .is_none(),
        "draft in deleted room must cascade away",
    );
    assert!(
        db::drafts::get(&t.chat, &t.alice_id, 1)
            .await
            .unwrap()
            .is_some(),
        "draft in surviving room must persist",
    );
}

// ----------------------------------------------------------------------
// Render-on-load: the room page handler reads the draft and the
// composer template renders it as the textarea inner content.
// ----------------------------------------------------------------------

// LC-173: the sidebar self block (own avatar + name) lives in the swappable
// #sidebar-self region the own-profile OOB fragment targets, so a profile edit
// refreshes it live in every tab. Pin the region + its contents so the
// extraction into partials/sidebar_self.html cannot silently regress.
#[tokio::test]
async fn sidebar_self_block_is_present_and_swappable() {
    let t = app().await;
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("id=\"sidebar-self\""),
        "sidebar self block must be in the swappable region the own-profile OOB fragment targets",
    );
    assert!(
        body.contains("id=\"own-avatar-wrapper\""),
        "own avatar (status trigger) must remain inside the self block",
    );
    assert!(body.contains("@alice"), "self block shows the username");
}

#[tokio::test]
async fn room_render_pre_populates_textarea_with_draft() {
    let t = app().await;
    db::drafts::upsert(&t.chat, &t.alice_id, 1, "in-progress thought")
        .await
        .unwrap();
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    // Composer textarea renders with the draft as inner content.
    assert!(
        body.contains("in-progress thought"),
        "rendered page must include the draft body in the textarea",
    );
}

// LC-171: the composer's Enter-to-submit handler must defer to an open
// mention/slash combobox so Enter never both submits the partial text and
// inserts the highlighted option. The guard is a single inline check in the
// textarea's onkeydown; this pins it so a refactor of the composer cannot
// silently drop it and reintroduce the double-fire. We assert the guard
// short-circuits BEFORE the submit call, since order is what makes it correct.
#[tokio::test]
async fn composer_enter_handler_defers_to_open_combobox() {
    let t = app().await;
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    // Assert the guard's two stable anchors independently (rather than the full
    // selector literal) so reordering the two popover clauses or reflowing
    // whitespace inside the selector does not break the test; what matters is
    // that both popovers' selected-option state is consulted and that the check
    // short-circuits BEFORE the submit call, since order is what makes it work.
    let mention_anchor = "#lc-mention-popover li[role=option][aria-selected=true]";
    let slash_anchor = "#lc-slash-popover li[role=option][aria-selected=true]";
    assert!(
        body.contains(mention_anchor),
        "composer Enter handler must consult the mention popover's selected option",
    );
    assert!(
        body.contains(slash_anchor),
        "composer Enter handler must consult the slash popover's selected option",
    );
    let guard_pos = body.find("[aria-selected=true]").unwrap();
    let submit_pos = body[guard_pos..]
        .find("requestSubmit()")
        .map(|p| guard_pos + p)
        .expect("composer must still submit on Enter");
    assert!(
        guard_pos < submit_pos,
        "the combobox guard must short-circuit before requestSubmit, or the double-fire returns",
    );
}

#[tokio::test]
async fn room_render_with_stale_draft_renders_empty_and_purges_row() {
    let t = app().await;
    db::drafts::upsert(&t.chat, &t.alice_id, 1, "abandoned 6 months ago")
        .await
        .unwrap();
    sqlx::query("UPDATE message_drafts SET updated_at = datetime('now', '-61 days') WHERE user_id = ? AND room_id = ?")
        .bind(&t.alice_id)
        .bind(1i64)
        .execute(&t.chat)
        .await
        .unwrap();

    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("abandoned 6 months ago"),
        "stale draft must not appear in the rendered composer",
    );

    // Load-bearing: the row is actually DELETED, not just hidden.
    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM message_drafts WHERE user_id = ? AND room_id = ?")
            .bind(&t.alice_id)
            .bind(1i64)
            .fetch_one(&t.chat)
            .await
            .unwrap();
    assert_eq!(
        row_count, 0,
        "stale draft row must be deleted by the render-time lazy cleanup",
    );
}

// ----------------------------------------------------------------------
// EXPLAIN QUERY PLAN on the race-guard SELECT. Pins the index choice:
// a future schema refactor that drops idx_messages_room (or replaces
// it with a less-selective index for this query) makes this test fail
// in CI rather than surfacing as production latency. The 5-second
// window already bounds the candidate set tightly at any realistic
// scale, but the query plan should still walk via an index, not a
// table scan.
// ----------------------------------------------------------------------

#[tokio::test]
async fn race_guard_query_plan_uses_an_index() {
    let t = app().await;
    let rows = sqlx::query(
        "EXPLAIN QUERY PLAN \
         SELECT 1 FROM messages \
         WHERE user_id = ? AND room_id = ? AND body = ? \
           AND created_at >= datetime('now', '-5 seconds') \
         LIMIT 1",
    )
    .bind(&t.alice_id)
    .bind(1i64)
    .bind("hi")
    .fetch_all(&t.chat)
    .await
    .unwrap();

    let plan: Vec<String> = rows.iter().map(|r| r.get::<String, _>("detail")).collect();
    let plan_text = plan.join(" | ");

    // Print the plan so a developer reading the test output can see
    // which index the planner actually picked. This is informational;
    // the assertion below is what gates the test.
    eprintln!("race-guard query plan: {plan_text}");

    // The SQLite planner output for an indexed lookup is "SEARCH ...
    // USING INDEX <name>". A plan that contains "SCAN" without
    // "USING INDEX" is a full table scan, which is what we want to
    // avoid as the table grows.
    assert!(
        plan_text.contains("USING INDEX") || plan_text.contains("USING COVERING INDEX"),
        "race-guard SELECT must walk via an index, not a full scan. plan: {plan_text}",
    );
}

// ----------------------------------------------------------------------
// LC-239: per-conversation draft indicator. The set query that feeds the
// sidebar badge, plus the page render asserting the badge appears next to a
// room with a draft and is absent (empty id-keyed span) otherwise.
// ----------------------------------------------------------------------

#[tokio::test]
async fn room_ids_with_drafts_returns_only_fresh_nonempty_rows() {
    let t = app().await;
    let r2 = private_room(&t.chat, "two").await;
    let r3 = private_room(&t.chat, "three").await;

    // Fresh drafts in rooms 1 and r2 for alice.
    db::drafts::upsert(&t.chat, &t.alice_id, 1, "a")
        .await
        .unwrap();
    db::drafts::upsert(&t.chat, &t.alice_id, r2, "b")
        .await
        .unwrap();
    // A stale draft in r3 must be excluded (mirrors the render-side freshness
    // rule) without being deleted here.
    db::drafts::upsert(&t.chat, &t.alice_id, r3, "old")
        .await
        .unwrap();
    sqlx::query("UPDATE message_drafts SET updated_at = datetime('now', '-61 days') WHERE user_id = ? AND room_id = ?")
        .bind(&t.alice_id)
        .bind(r3)
        .execute(&t.chat)
        .await
        .unwrap();
    // Bob's draft in room 1 must not leak into alice's set.
    db::drafts::upsert(&t.chat, &t.bob_id, 1, "bob")
        .await
        .unwrap();

    let set = db::drafts::room_ids_with_drafts(&t.chat, &t.alice_id, 60)
        .await
        .unwrap();
    assert!(set.contains(&1), "fresh draft in room 1 must be in the set");
    assert!(set.contains(&r2), "fresh draft in r2 must be in the set");
    assert!(!set.contains(&r3), "stale draft must be excluded");
    assert_eq!(set.len(), 2, "exactly the two fresh rooms, no leakage");

    // The stale row is excluded from the set but NOT deleted by this read
    // (lazy purge stays on the per-room render path).
    assert!(
        db::drafts::get(&t.chat, &t.alice_id, r3)
            .await
            .unwrap()
            .is_some(),
        "room_ids_with_drafts must not delete the stale row",
    );
}

#[tokio::test]
async fn sidebar_shows_draft_pencil_for_room_with_draft() {
    let t = app().await;
    db::drafts::upsert(&t.chat, &t.alice_id, 1, "half-typed")
        .await
        .unwrap();
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    // The id-keyed badge span is always present (so a later DraftChanged OOB
    // swap has a target); with a draft it carries the pencil glyph.
    assert!(
        body.contains("id=\"lc-draft-1\""),
        "sidebar row for room 1 must carry the draft badge target span",
    );
    assert!(
        body.contains("&#9998;"),
        "a room with a draft must render the pencil draft indicator",
    );
}

#[tokio::test]
async fn sidebar_omits_draft_pencil_when_no_draft() {
    let t = app().await;
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("id=\"lc-draft-1\""),
        "the empty id-keyed badge span must still render as the OOB swap target",
    );
    assert!(
        !body.contains("&#9998;"),
        "no draft anywhere means no pencil indicator is rendered",
    );
}

// ----------------------------------------------------------------------
// LC-242: jump-to-latest pill renders (hidden) on the room page with its
// localized strings. Counter/scroll behaviour is JS-only and covered by the
// LC-243 QA pass; this just pins that the markup and i18n keys are present.
// ----------------------------------------------------------------------

#[tokio::test]
async fn room_page_renders_jump_to_latest_pill_hidden() {
    let t = app().await;
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("id=\"lc-jump-latest\""),
        "room page must render the jump-to-latest pill",
    );
    // Starts hidden; auto_scroll.html reveals it on scroll-up.
    assert!(
        body.contains("class=\"hidden absolute"),
        "pill must start hidden",
    );
    // i18n keys resolve (not echoed literally) for the aria-label and the
    // localized count templates the JS substitutes %n% into.
    assert!(
        body.contains("aria-label=\"Jump to latest message\""),
        "pill aria-label i18n key must resolve",
    );
    assert!(
        body.contains("data-lc-jump-many=\"%n% new messages\""),
        "pill carries the localized many-form template with the %n% token",
    );
}

// ----------------------------------------------------------------------
// LC-244: message-list date separators. Day boundaries are UTC; the first
// message and each day change render a divider. Live insertion is JS-only
// (LC-245 QA pass); these pin the server render + the data-lc-day attribute.
// ----------------------------------------------------------------------

async fn set_created_at(chat: &SqlitePool, message_id: i64, created_at: &str) {
    sqlx::query("UPDATE messages SET created_at = ? WHERE id = ?")
        .bind(created_at)
        .bind(message_id)
        .execute(chat)
        .await
        .unwrap();
}

#[tokio::test]
async fn message_list_renders_day_separator_on_day_change() {
    let t = app().await;
    // An old message (fixed absolute date) followed by a fresh one (today UTC,
    // the default created_at). Two distinct UTC days => two dividers.
    let old = insert_message_at(&t.chat, 1, &t.alice_id, "old", 0).await;
    set_created_at(&t.chat, old, "2020-01-01 10:00:00").await;
    insert_message_at(&t.chat, 1, &t.alice_id, "fresh", 0).await;

    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("data-lc-day-divider"),
        "a day separator must render",
    );
    // Absolute date for the old day (appears once, for its single divider).
    assert_eq!(
        body.matches("January 1, 2020").count(),
        1,
        "old message's day shows an absolute-date separator exactly once",
    );
    // The fresh message's day is today UTC -> localized "Today".
    assert!(body.contains("Today"), "today's day shows the Today label");
    // Each row carries its UTC day for the client-side live-divider hook.
    assert!(
        body.contains("data-lc-day=\"2020-01-01\""),
        "message row carries its UTC day",
    );
}

#[tokio::test]
async fn message_list_no_duplicate_separator_within_a_day() {
    let t = app().await;
    // Two messages on the SAME UTC day => exactly one divider for that day.
    let a = insert_message_at(&t.chat, 1, &t.alice_id, "first", 0).await;
    set_created_at(&t.chat, a, "2020-03-04 09:00:00").await;
    let b = insert_message_at(&t.chat, 1, &t.alice_id, "second", 0).await;
    set_created_at(&t.chat, b, "2020-03-04 11:30:00").await;

    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.matches("March 4, 2020").count(),
        1,
        "same-day messages share one separator, not one per message",
    );
}

// ----------------------------------------------------------------------
// LC-248: Arrow-Up in an empty composer edits the last own message. The
// find-last-and-click + Esc-cancel are JS-only (LC-249 QA pass); these pin
// that the shortcut script ships and the edit form auto-focuses.
// ----------------------------------------------------------------------

#[tokio::test]
async fn room_page_ships_arrowup_edit_last_shortcut() {
    let t = app().await;
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("__lcEditLast"),
        "composer must ship the Arrow-Up edit-last handler",
    );
}

#[tokio::test]
async fn edit_form_autofocuses_its_input() {
    let t = app().await;
    let mid = insert_message_at(&t.chat, 1, &t.alice_id, "editable", 0).await;
    let (status, body) = send(
        &t.app,
        &t.alice_session,
        Method::GET,
        &format!("/messages/{mid}/edit"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("autofocus"),
        "edit form input must autofocus so the editor takes focus on open",
    );
}

// ----------------------------------------------------------------------
// LC-252: keyboard-shortcuts cheat-sheet. The `?` keybind, open/close, and
// focus trap are JS-only; this pins that the singleton modal + its `?`
// handler ship in the persistent shell and the sidebar exposes the link.
// ----------------------------------------------------------------------

#[tokio::test]
async fn room_page_ships_shortcuts_overlay() {
    let t = app().await;
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("data-lc-shortcuts-modal"),
        "page must ship the keyboard-shortcuts modal markup",
    );
    assert!(
        body.contains("__lcOpenShortcuts"),
        "page must ship the `?` keybind / open handler",
    );
    assert!(
        body.contains("lc-shortcuts-modal-title"),
        "shortcuts modal must be labelled for assistive tech",
    );
}

// ----------------------------------------------------------------------
// LC-254: composer Markdown formatting toolbar. The wrap/link transforms and
// Ctrl/Cmd+B/I are JS-only; this pins that the toolbar markup + handler ship.
// ----------------------------------------------------------------------

#[tokio::test]
async fn room_page_ships_formatting_toolbar() {
    let t = app().await;
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("data-lc-fmt=\"bold\""),
        "composer must ship the formatting toolbar buttons",
    );
    assert!(
        body.contains("__lcFormat"),
        "composer must ship the formatting handler",
    );
}

// ----------------------------------------------------------------------
// LC-256: collapse long messages. The fold is JS-measured (auto_scroll.html);
// the server always ships the marker + hidden toggle, so this pins that a
// non-empty message body carries the collapse markup.
// ----------------------------------------------------------------------

#[tokio::test]
async fn message_body_ships_collapse_markup() {
    let t = app().await;
    insert_message_at(&t.chat, 1, &t.alice_id, "a long-enough body", 0).await;
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("data-lc-collapse"),
        "message body must carry the collapse marker",
    );
    assert!(
        body.contains("data-lc-collapse-toggle"),
        "message must ship the Show more/less toggle",
    );
}

// ----------------------------------------------------------------------
// LC-262: image lightbox. The viewer is JS-driven; the server marks image
// attachment anchors with data-lc-lightbox and ships the singleton modal.
// ----------------------------------------------------------------------

#[tokio::test]
async fn image_attachment_ships_lightbox_markup() {
    let t = app().await;
    let mid = insert_message_at(&t.chat, 1, &t.alice_id, "look at this", 0).await;
    let up = db::uploads::insert_upload(
        &t.chat,
        &t.alice_id,
        "pic.png",
        "image/png",
        1234,
        "a/b/hash.png",
        None,
    )
    .await
    .unwrap();
    db::uploads::link_upload_to_message(&t.chat, up, mid)
        .await
        .unwrap();

    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("/api/files/{up}")),
        "the image attachment must render",
    );
    assert!(
        body.contains("data-lc-lightbox target="),
        "the image anchor must be lightbox-enabled",
    );
    assert!(
        body.contains("data-lc-lightbox-modal"),
        "the page must ship the lightbox modal",
    );
}

// ----------------------------------------------------------------------
// LC-270: collapse-sidebar-to-rail. The toggle + persistence are JS/CSS; this
// pins that the rail ships the toggle and the bootstrap exposes the flipper.
// ----------------------------------------------------------------------

#[tokio::test]
async fn page_ships_sidebar_collapse_toggle() {
    let t = app().await;
    let (status, body) = send(&t.app, &t.alice_session, Method::GET, "/room/1", "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("data-lc-sidebar-toggle"),
        "enclave rail must ship the sidebar collapse toggle",
    );
    assert!(
        body.contains("__lcToggleSidebar"),
        "page must ship the sidebar collapse flipper",
    );
}
