//! LC-84 integration: per-room moderator overrides.
//!
//! Covers the happy path (an org-Moderator-less user with a Moderator
//! override can delete another user's message in that room), the
//! authorization gate on the management endpoint (a plain member cannot
//! grant), and an audit-log spot-check (a `room_role_grant` row lands
//! in `mod_actions`).
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() -> &'static str {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("lc-room-rbac-tests-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("create test data dir");
            db::set_data_dir(p.to_string_lossy().to_string());
            p.to_string_lossy().to_string()
        })
        .as_str()
}

mod common;

struct TestApp {
    app: Router,
    admin_session: String,
    member_session: String,
    admin_id: String,
    member_id: String,
    chat: SqlitePool,
}

async fn app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    let member_id = db::auth::create_user(&auth, "member", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
        .bind(&member_id)
        .execute(&auth)
        .await
        .unwrap();
    let admin_session = db::auth::create_session(&auth, &admin_id).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let chat_for_test = chat.clone();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
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
        bunyip_sso: None,
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        admin_session,
        member_session,
        admin_id,
        member_id,
        chat: chat_for_test,
    }
}

async fn send(app: &Router, sess: &str, method: Method, uri: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

/// LC-677: an htmx toggle POST (assistant/digest/stage) returns the re-rendered
/// switch reflecting the NEW state - the switch flips, and the hidden "next
/// value" flips too - so the UI updates in place without a page reload, and a
/// second click actually toggles back.
#[tokio::test]
async fn toggle_post_rerenders_the_switch_with_the_new_state() {
    let t = app().await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/room/1/assistant")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("session={}", t.admin_session))
        .header("hx-request", "true")
        .body(Body::from("enabled=1"))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body =
        String::from_utf8_lossy(&to_bytes(res.into_body(), 1 << 20).await.unwrap()).into_owned();
    assert!(
        body.contains(r#"aria-checked="true""#),
        "switch shows the new ON state: {body}"
    );
    assert!(
        body.contains(r#"name="enabled" value="0""#),
        "hidden next-value flips to OFF so a second click turns it back off: {body}"
    );
    assert!(
        body.contains(r#"hx-post="/room/1/assistant""#),
        "re-rendered form posts back to the same route: {body}"
    );
    assert!(
        db::chat::get_room_assistant_enabled(&t.chat, 1)
            .await
            .unwrap(),
        "persisted"
    );
}

#[tokio::test]
async fn member_cannot_delete_others_messages_without_override() {
    let t = app().await;
    // Admin posts a message in room 1.
    let admin_msg_id = post_message_via_db(&t.chat, 1, &t.admin_id, "hi").await;
    // Member tries to delete it.
    let status = send(
        &t.app,
        &t.member_session,
        Method::DELETE,
        &format!("/messages/{admin_msg_id}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn member_with_moderator_override_can_delete_others_messages() {
    let t = app().await;
    let admin_msg_id = post_message_via_db(&t.chat, 1, &t.admin_id, "hi").await;
    // Admin grants member moderator override on room 1.
    let body = format!("user_id={}&role=moderator", t.member_id);
    let status = send(
        &t.app,
        &t.admin_session,
        Method::POST,
        "/room/1/moderators",
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    // Member now deletes the admin's message.
    let status = send(
        &t.app,
        &t.member_session,
        Method::DELETE,
        &format!("/messages/{admin_msg_id}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn plain_member_cannot_grant_override() {
    let t = app().await;
    let body = format!("user_id={}&role=moderator", t.admin_id);
    let status = send(
        &t.app,
        &t.member_session,
        Method::POST,
        "/room/1/moderators",
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn grant_revoke_writes_audit_log_rows() {
    let t = app().await;
    let body = format!("user_id={}&role=moderator", t.member_id);
    assert_eq!(
        send(
            &t.app,
            &t.admin_session,
            Method::POST,
            "/room/1/moderators",
            &body
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(
            &t.app,
            &t.admin_session,
            Method::DELETE,
            &format!("/room/1/moderators/{}", t.member_id),
            ""
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let actions = db::moderation::list_mod_actions(&t.chat).await.unwrap();
    let grant = actions
        .iter()
        .find(|a| a.action == "room_role_grant")
        .expect("grant action recorded");
    let revoke = actions
        .iter()
        .find(|a| a.action == "room_role_revoke")
        .expect("revoke action recorded");
    assert_eq!(grant.target_user, t.member_id);
    assert_eq!(grant.actor_user, t.admin_id);
    assert_eq!(grant.room_id, Some(1));
    assert_eq!(revoke.target_user, t.member_id);
    assert_eq!(revoke.actor_user, t.admin_id);
    assert_eq!(revoke.room_id, Some(1));
}

// ----------------------------------------------------------------------
// LC-599: the DM timeline used to compute `can_delete` from the org role
// alone, while the delete endpoint (shared with rooms) resolves the
// effective role. A moderator-by-override therefore got no Delete item on a
// message the endpoint would have let them delete. These two tests pin the
// template to the endpoint: the negative one proves the affordance is still
// withheld from a plain member, so the positive one is not vacuous.
// ----------------------------------------------------------------------

/// Fetch a page as `sess`, asserting 200 and returning the body.
async fn get_body(app: &Router, sess: &str, uri: &str) -> String {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "GET {uri}");
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Open the member<->admin DM (the handler creates the room on first view)
/// and return its room id, so the grant can target it.
async fn open_dm_room(t: &TestApp) -> i64 {
    get_body(&t.app, &t.member_session, &format!("/dm/{}", t.admin_id)).await;
    db::chat::find_dm_room(&t.chat, &t.member_id, &t.admin_id)
        .await
        .unwrap()
        .expect("viewing a DM creates the room")
        .id
}

#[tokio::test]
async fn dm_hides_delete_from_a_plain_member() {
    let t = app().await;
    let dm_id = open_dm_room(&t).await;
    let msg_id = post_message_via_db(&t.chat, dm_id, &t.admin_id, "peer message").await;

    let body = get_body(&t.app, &t.member_session, &format!("/dm/{}", t.admin_id)).await;
    assert!(
        body.contains("peer message"),
        "the message must actually render, or the negative assertion below is vacuous"
    );
    assert!(
        !body.contains(&format!(r#"hx-delete="/messages/{msg_id}""#)),
        "a plain member must not be offered Delete on the peer's message"
    );
}

#[tokio::test]
async fn dm_offers_delete_to_a_moderator_by_room_override() {
    let t = app().await;
    let dm_id = open_dm_room(&t).await;
    let msg_id = post_message_via_db(&t.chat, dm_id, &t.admin_id, "peer message").await;

    // The DM room has no enclave, so `require_can_manage` falls through to the
    // site role: an org admin can grant an override here.
    let grant = format!("user_id={}&role=moderator", t.member_id);
    assert_eq!(
        send(
            &t.app,
            &t.admin_session,
            Method::POST,
            &format!("/room/{dm_id}/moderators"),
            &grant
        )
        .await,
        StatusCode::SEE_OTHER
    );

    let body = get_body(&t.app, &t.member_session, &format!("/dm/{}", t.admin_id)).await;
    assert!(
        body.contains(&format!(r#"hx-delete="/messages/{msg_id}""#)),
        "a moderator-by-override must see Delete on the peer's message"
    );

    // ...and the endpoint agrees, which is the invariant that drifted.
    assert_eq!(
        send(
            &t.app,
            &t.member_session,
            Method::DELETE,
            &format!("/messages/{msg_id}"),
            ""
        )
        .await,
        StatusCode::OK
    );
}

/// Bypass the HTTP handler and write a message row directly so the test
/// has a target id without depending on the request/response shape of
/// `POST /room/{id}/messages` (which returns OOB-friendly HTML, not the
/// new id). Uses the same chat pool the AppState shares with the app.
async fn post_message_via_db(chat: &SqlitePool, room_id: i64, user_id: &str, body: &str) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO messages (room_id, user_id, body) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(room_id)
    .bind(user_id)
    .bind(body)
    .fetch_one(chat)
    .await
    .unwrap();
    id
}
