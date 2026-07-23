use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

async fn open_pool(name: &str) -> SqlitePool {
    common::pool(name).await
}

async fn app_with_user_in_general() -> (Router, String, String, SqlitePool) {
    app_with_user(true).await
}

/// LC-600: `/` has two branches and they need different fixtures. `show_dashboard`
/// (routes/home.rs) is `!room_counts.is_empty() || !dm_counts.is_empty()`, so a
/// user with any accessible room gets the LC-575 dashboard and only a user with
/// none gets the LC-372 onboarding empty state.
///
/// LC-603: this fixture used to need `DELETE FROM rooms` to reach the empty
/// state, because `list_room_unread_counts` read "public" as public to the
/// whole instance and the migration seeds the public rooms `general` and
/// `random`. Every user on a seeded instance therefore had an accessible room
/// and the LC-372 branch was effectively dead in production.
///
/// LC-604 replaced that predicate with `accessible_rooms_sql`, which requires
/// enclave membership before a channel counts as visible, and LC-516 had
/// already removed the auto-join-to-General behaviour. So a fresh non-admin
/// user is now a member of no enclave, sees no rooms, and reaches the
/// onboarding page on a fully seeded instance. The `DELETE FROM rooms` crutch
/// is gone: `in_general: false` now only skips the backfill and leaves the user
/// a plain member, which is exactly the state a real new signup is in.
async fn app_with_user(in_general: bool) -> (Router, String, String, SqlitePool) {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let user_id = db::auth::create_user(&auth, "tester", "h").await.unwrap();
    let role = if in_general { "admin" } else { "user" };
    sqlx::query("UPDATE users SET role=?, totp_enabled=1 WHERE id=?")
        .bind(role)
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    if in_general {
        db::enclave::backfill_general_membership(&auth, &chat)
            .await
            .unwrap();
    }

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
        chat,
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg: bg.clone(),
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: std::sync::Arc::new(lets_chat::push::MockPushClient::default()),
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
    (routes::build_router(state), session, user_id, auth)
}

/// Fetch `/` as the given session and return the rendered body.
async fn get_home(app: &Router, sess: &str) -> String {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("cookie", format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn home_renders_welcome_when_no_cookie() {
    // LC-600: a user with no accessible room - the only case that still reaches
    // the onboarding empty state since LC-575.
    let (app, sess, _id, _) = app_with_user(false).await;
    let s = get_home(&app, &sess).await;
    assert!(s.contains("Welcome"), "should render welcome page");
}

// LC-575: a user who has an accessible room gets the dashboard instead of the
// onboarding empty state. This branch was previously untested, which is how the
// two tests below kept asserting the pre-LC-575 page (LC-600).
#[tokio::test]
async fn home_renders_dashboard_when_user_has_rooms() {
    let (app, sess, _id, _) = app_with_user_in_general().await;
    let s = get_home(&app, &sess).await;
    assert!(s.contains("Catch up"), "renders the dashboard heading");
    assert!(
        s.contains("Unread channels"),
        "renders the catch-up card heading"
    );
    assert!(s.contains("Mentions"), "renders the mentions card");
    assert!(s.contains("Drafts"), "renders the drafts card");
    assert!(
        !s.contains("start something new"),
        "must not fall back to the onboarding empty state"
    );
}

// LC-372: the welcome empty state renders the quick-action cards (Start a DM,
// Discover/create an enclave, View invitations) and the supporting subtitle.
#[tokio::test]
async fn home_welcome_renders_quick_actions() {
    let (app, sess, _id, _) = app_with_user(false).await;
    let s = get_home(&app, &sess).await;
    assert!(
        s.contains("start something new"),
        "renders the empty-state subtitle"
    );
    assert!(
        s.contains("Start a direct message"),
        "renders the Start-a-DM quick action"
    );
    assert!(
        s.contains("href=\"/enclaves/discover\""),
        "renders the discover/create enclave action with its link"
    );
    assert!(
        s.contains("href=\"/invitations\""),
        "renders the view-invitations action with its link"
    );
}

#[tokio::test]
async fn home_redirects_to_last_visited_room_when_safe() {
    let (app, sess, id, auth) = app_with_user_in_general().await;
    // LC-621: the last-visited redirect is opt-in now (the default landing is the
    // Home dashboard), so set the "last-room" preference to exercise the redirect.
    db::auth::set_user_home_landing(&auth, &id, Some("last-room"))
        .await
        .unwrap();
    // The migration seeded 'general' (id=1) and 'random' (id=2) into the General enclave.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(
            "cookie",
            format!("session={sess}; lets_chat_last_visited=/room/1"),
        )
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get("location").unwrap().to_str().unwrap(),
        "/room/1"
    );
}

#[tokio::test]
async fn home_ignores_malformed_cookie() {
    let (app, sess, _id, _) = app_with_user_in_general().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(
            "cookie",
            format!("session={sess}; lets_chat_last_visited=/admin/users"),
        )
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn home_ignores_inaccessible_room_cookie() {
    let (app, sess, _id, _) = app_with_user_in_general().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(
            "cookie",
            format!("session={sess}; lets_chat_last_visited=/room/999999"),
        )
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn get_room_sets_last_visited_cookie() {
    let (app, sess, _id, _) = app_with_user_in_general().await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/room/1")
        .header("cookie", format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie_header = res.headers().get_all("set-cookie");
    let mut found = false;
    for v in cookie_header {
        if v.to_str()
            .unwrap()
            .contains("lets_chat_last_visited=/room/1")
        {
            found = true;
        }
    }
    assert!(
        found,
        "Set-Cookie header must include the last_visited entry"
    );
}
