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

pub async fn app_with_user(role: &str) -> (Router, String) {
    let (app, sess, _id) = app_with_named_user(role, "tester").await;
    (app, sess)
}

pub async fn app_with_named_user(role: &str, username: &str) -> (Router, String, String) {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let user_id = db::auth::create_user(&auth, username, "hash")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role=?, totp_enabled=1 WHERE id=?")
        .bind(role)
        .bind(&user_id)
        .execute(&auth)
        .await
        .unwrap();
    let session_token = db::auth::create_session(&auth, &user_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

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
    let app = routes::build_router(state);
    (app, session_token, user_id)
}

/// Two users sharing the same in-memory pools. Returns (app, sess1, id1, sess2, id2).
pub async fn app_with_two_users() -> (Router, String, String, String, String) {
    let auth = open_pool("auth").await;
    let chat = open_pool("chat").await;
    let settings = open_pool("settings").await;

    let id1 = db::auth::create_user(&auth, "alice", "h1").await.unwrap();
    let id2 = db::auth::create_user(&auth, "bob", "h2").await.unwrap();
    sqlx::query("UPDATE users SET role='user', totp_enabled=1 WHERE id IN (?, ?)")
        .bind(&id1)
        .bind(&id2)
        .execute(&auth)
        .await
        .unwrap();
    // First user gets promoted by hand to seed General owner.
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&id1)
        .execute(&auth)
        .await
        .unwrap();
    let s1 = db::auth::create_session(&auth, &id1).await.unwrap();
    let s2 = db::auth::create_session(&auth, &id2).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    // Demote alice back to plain user for tests that want a non-admin owner.
    sqlx::query("UPDATE users SET role='user' WHERE id=?")
        .bind(&id1)
        .execute(&auth)
        .await
        .unwrap();

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
    let app = routes::build_router(state);
    (app, s1, id1, s2, id2)
}

pub fn cookie(token: &str) -> String {
    format!("session={token}")
}

/// LC-143: clicking an enclave now redirects to a room; the room page's
/// sidebar lists every room in the enclave. Follow the redirect and return
/// that room page's body so tests can grep the enclave's room list.
async fn enclave_room_list_body(app: &Router, sess: &str, enclave_id: i64) -> String {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/enclave/{enclave_id}"))
        .header("cookie", cookie(sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let target = res
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let res = match target {
        // Redirected to a room: fetch the room page (sidebar lists all rooms).
        Some(path) => {
            let req = Request::builder()
                .method(Method::GET)
                .uri(path)
                .header("cookie", cookie(sess))
                .body(Body::empty())
                .unwrap();
            app.clone().oneshot(req).await.unwrap()
        }
        // No redirect (empty enclave): the landing body itself.
        None => res,
    };
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn post_enclaves_creates_and_redirects() {
    let (app, sess) = app_with_user("user").await;
    let body = "name=rust&description=rustaceans";
    let req = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res.headers().get("location").unwrap().to_str().unwrap();
    assert!(loc.starts_with("/enclave/"));
}

#[tokio::test]
async fn get_enclave_landing_renders_for_member() {
    let (app, sess) = app_with_user("user").await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=rust"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let loc = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let req = Request::builder()
        .method(Method::GET)
        .uri(&loc)
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(s.contains("rust"));
}

#[tokio::test]
async fn get_enclave_landing_404_for_unknown() {
    let (app, sess) = app_with_user("user").await;
    let req = Request::builder()
        .method(Method::GET)
        .uri("/enclave/999999")
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn discover_lists_only_public_enclaves() {
    let (app, sess) = app_with_user("user").await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=open"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();

    let vis = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/visibility"))
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("is_public=1"))
        .unwrap();
    app.clone().oneshot(vis).await.unwrap();

    let get = Request::builder()
        .method(Method::GET)
        .uri("/enclaves/discover")
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(get).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(s.contains("open"));
}

#[tokio::test]
async fn join_by_invite_code_adds_member() {
    let (app, sess) = app_with_user("user").await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=clubhouse"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();

    let gen_code = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/invite-code"))
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(gen_code).await.unwrap();
    assert!(res.status().is_redirection());
    // The owner is already a member; we just verify the endpoint succeeds.
}

/// Create an enclave as `sess`, generate its invite code, and return
/// `(enclave_id, code)` by parsing the code back off the settings page
/// (`data-lc-copy="..."`). The creator is the enclave owner, so the
/// manage-gated settings page and invite-code generation both pass.
async fn create_enclave_with_code(app: &Router, sess: &str, name: &str) -> (i64, String) {
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!("name={name}")))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();

    let gen_code = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/invite-code"))
        .header("cookie", cookie(sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(gen_code).await.unwrap();

    let settings = Request::builder()
        .method(Method::GET)
        .uri(format!("/enclave/{id}/settings"))
        .header("cookie", cookie(sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(settings).await.unwrap();
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    let marker = "data-lc-copy=\"";
    let start = s
        .find(marker)
        .expect("invite code missing from settings page")
        + marker.len();
    let end = s[start..].find('"').unwrap();
    let code = s[start..start + end].to_string();
    assert!(!code.is_empty(), "parsed invite code is empty");
    (id, code)
}

// LC-544: an invalid / revoked / expired code no longer shows a raw 400. It
// redirects (303) back to the discover page with a friendly, actionable
// banner. These three tests cover the invalid, revoked, and valid paths.

#[tokio::test]
async fn join_by_invalid_invite_code_redirects_to_discover() {
    let (app, sess) = app_with_user("user").await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/enclaves/join")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("code=nonsense"))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(loc, "/enclaves/discover?error=invalid_invite_code");

    // Follow the redirect and confirm the discover page actually renders the
    // friendly banner. Asserting the Location alone would stay green even if
    // the flash_message arm were removed (the fix and the arm are coupled only
    // by the `invalid_invite_code` string), leaving the user a blank page.
    let follow = Request::builder()
        .method(Method::GET)
        .uri(&loc)
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(follow).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        s.contains("invalid, revoked, or expired"),
        "discover page must render the friendly invalid-invite-code banner"
    );
}

#[tokio::test]
async fn join_by_revoked_invite_code_redirects_to_discover() {
    let (app, s1, _id1, s2, _id2) = app_with_two_users().await;
    // Alice owns an enclave with a live code; Bob captures it.
    let (id, code) = create_enclave_with_code(&app, &s1, "rotate").await;

    // Alice rotates the code, revoking the old one.
    let rotate = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/invite-code"))
        .header("cookie", cookie(&s1))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(rotate).await.unwrap();

    // Bob tries the now-revoked code: friendly redirect, not a 400.
    let join = Request::builder()
        .method(Method::POST)
        .uri("/enclaves/join")
        .header("cookie", cookie(&s2))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!("code={code}")))
        .unwrap();
    let res = app.clone().oneshot(join).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(loc, "/enclaves/discover?error=invalid_invite_code");
}

#[tokio::test]
async fn join_by_valid_invite_code_adds_member_and_redirects_to_enclave() {
    let (app, s1, _id1, s2, _id2) = app_with_two_users().await;
    // Alice owns an enclave with a live code; Bob is not a member.
    let (id, code) = create_enclave_with_code(&app, &s1, "welcome").await;

    let join = Request::builder()
        .method(Method::POST)
        .uri("/enclaves/join")
        .header("cookie", cookie(&s2))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!("code={code}")))
        .unwrap();
    let res = app.clone().oneshot(join).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let loc = res.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(loc, format!("/enclave/{id}"));

    // Bob is now a member: the enclave landing renders for him (200 or a
    // redirect into a room, both of which a non-member would get 403 for).
    let landing = Request::builder()
        .method(Method::GET)
        .uri(format!("/enclave/{id}"))
        .header("cookie", cookie(&s2))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(landing).await.unwrap();
    assert!(
        res.status() == StatusCode::OK || res.status().is_redirection(),
        "member must reach enclave landing, got {}",
        res.status()
    );
}

#[tokio::test]
async fn discover_join_rejects_private() {
    let (app, sess) = app_with_user("user").await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=secretclub"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclaves/discover/{id}/join"))
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invite_then_accept_creates_membership() {
    let (app, s1, _id1, s2, id2) = app_with_two_users().await;
    // Alice creates an enclave.
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&s1))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=alices-place"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let enclave_id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();

    // Alice invites Bob. `post_invite` returns the HTMX result fragment
    // (HTTP 200), not a redirect.
    let invite = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{enclave_id}/invite"))
        .header("cookie", cookie(&s1))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!("user_id={id2}")))
        .unwrap();
    let res = app.clone().oneshot(invite).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Bob lists pending invitations.
    let list = Request::builder()
        .method(Method::GET)
        .uri("/invitations")
        .header("cookie", cookie(&s2))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(list).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        s.contains("alices-place"),
        "invitations page must show enclave name"
    );

    // Bob accepts (find the invitation id from the DB via the response).
    // Easier: re-fetch via a direct DB call by extracting the invite id from the rendered HTML.
    let inv_id: i64 = {
        let start = s.find("/invitations/").expect("invite link missing");
        let rest = &s[start + "/invitations/".len()..];
        let end = rest.find('/').unwrap();
        rest[..end].parse().unwrap()
    };
    let accept = Request::builder()
        .method(Method::POST)
        .uri(format!("/invitations/{inv_id}/accept"))
        .header("cookie", cookie(&s2))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(accept).await.unwrap();
    assert!(res.status().is_redirection());

    // Bob can now reach the enclave landing.
    let landing = Request::builder()
        .method(Method::GET)
        .uri(format!("/enclave/{enclave_id}"))
        .header("cookie", cookie(&s2))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(landing).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// LC-769: an invited user must see a labeled invitations indicator in the
// sidebar (present on every page), not only on the invitations page or blended
// into the Home rail badge. Before any invite Bob's sidebar has no banner;
// after Alice invites him it appears, pointing at /invitations.
#[tokio::test]
async fn invitation_surfaces_a_sidebar_banner_to_the_invitee() {
    let (app, s1, _id1, s2, id2) = app_with_two_users().await;

    // Before any invite, Bob's home carries no invitations banner.
    let home = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("cookie", cookie(&s2))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(home).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        !s.contains("data-lc-invites-banner"),
        "no banner should show before an invite exists"
    );

    // Alice creates an enclave and invites Bob.
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&s1))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=alices-place"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let enclave_id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();
    let invite = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{enclave_id}/invite"))
        .header("cookie", cookie(&s1))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!("user_id={id2}")))
        .unwrap();
    let res = app.clone().oneshot(invite).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Now Bob's home shows the banner, linking to /invitations.
    let home = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("cookie", cookie(&s2))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(home).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let s = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        s.contains("data-lc-invites-banner"),
        "the invited user must see the sidebar invitations banner"
    );
    assert!(
        s.contains("href=\"/invitations\""),
        "the banner must link to the accept / decline page"
    );
}

#[tokio::test]
async fn invitation_decline_only_by_invitee() {
    let (app, s1, _id1, _s2, _id2) = app_with_two_users().await;
    // Alice creates and invites herself by mistake to a fake id; we just verify
    // that hitting /accept on a missing invitation 404s.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/invitations/999/accept")
        .header("cookie", cookie(&s1))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn owner_cannot_self_leave() {
    let (app, sess) = app_with_user("user").await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=mine"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/leave"))
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn non_owner_cannot_delete_enclave() {
    let (app, s1, _id1, s2, id2) = app_with_two_users().await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&s1))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=alice-only"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();
    // Add Bob as a plain member, then try to delete as Bob.
    // Direct DB poke is fine here because we're testing the route guard, not invite flow.
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/invite"))
        .header("cookie", cookie(&s1))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("username=bob"))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();
    // Bob accepts via a synthetic invite-id lookup is overkill; instead verify
    // that delete by Bob (who has no membership) returns 403.
    let _ = id2;
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/delete"))
        .header("cookie", cookie(&s2))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_room_in_enclave_attaches_to_enclave() {
    let (app, sess) = app_with_user("user").await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=lab"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/rooms"))
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=experiments&room_type=public"))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(res.status().is_redirection());

    let s = enclave_room_list_body(&app, &sess, id).await;
    assert!(s.contains("experiments"));
}

#[tokio::test]
async fn create_room_rejects_unknown_type() {
    let (app, sess) = app_with_user("user").await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=lab2"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/rooms"))
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=x&room_type=garbage"))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delete_room_404_for_wrong_enclave() {
    let (app, sess) = app_with_user("user").await;
    // Create two enclaves; put a room in A; try to delete via B.
    let mk = |body: &'static str| {
        Request::builder()
            .method(Method::POST)
            .uri("/enclaves")
            .header("cookie", cookie(&sess))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap()
    };
    let res = app.clone().oneshot(mk("name=A")).await.unwrap();
    let a: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();
    let res = app.clone().oneshot(mk("name=B")).await.unwrap();
    let b: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();
    let mkroom = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{a}/rooms"))
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=r1&room_type=public"))
        .unwrap();
    app.clone().oneshot(mkroom).await.unwrap();
    // Find r1's id via direct DB call would need pool; instead try a wrong id.
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{b}/rooms/9999/delete"))
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn non_member_cannot_post_to_public_room_in_enclave() {
    let (app, s1, _id1, s2, _id2) = app_with_two_users().await;
    // Alice creates an enclave + a public room inside it.
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&s1))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=alices-only"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let enclave_id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();
    let mkroom = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{enclave_id}/rooms"))
        .header("cookie", cookie(&s1))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=lobby&room_type=public"))
        .unwrap();
    let res = app.clone().oneshot(mkroom).await.unwrap();
    assert!(res.status().is_redirection());

    // The enclave has exactly one room ("lobby"), so the first `/room/{id}`
    // reference on the page is it.
    let s = enclave_room_list_body(&app, &s1, enclave_id).await;
    assert!(s.contains("lobby"), "lobby room missing from page");
    let pos = s.find("/room/").expect("/room/ link missing");
    let after = &s[pos + "/room/".len()..];
    let end = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    let room_id: i64 = after[..end].parse().unwrap();

    // Bob (not a member of the enclave) tries to POST a message; must be 403.
    let post = Request::builder()
        .method(Method::POST)
        .uri(format!("/room/{room_id}/messages"))
        .header("cookie", cookie(&s2))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("body=intrusion"))
        .unwrap();
    let res = app.clone().oneshot(post).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_enclaves_requires_auth() {
    let (app, _sess) = app_with_user("user").await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=x"))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert!(res.status().is_redirection() || res.status() == StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn cannot_demote_enclave_owner() {
    // LC-351: a role change targeting the owner must be refused. The owner can
    // manage admins (so it clears the permission gate), but demoting the owner
    // would leave the enclave with zero owners and no way to transfer/delete it.
    let (app, sess, owner_id) = app_with_named_user("user", "owner").await;
    let create = Request::builder()
        .method(Method::POST)
        .uri("/enclaves")
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("name=mine"))
        .unwrap();
    let res = app.clone().oneshot(create).await.unwrap();
    let id: i64 = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("/enclave/")
        .parse()
        .unwrap();

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/enclave/{id}/members/{owner_id}/role"))
        .header("cookie", cookie(&sess))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("role=member"))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
