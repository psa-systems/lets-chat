//! LC-62: integration tests for the `/scheduled` HTTP routes.
//!
//! Uses the shared `common::pool(...)` migration helper (sqlx::migrate!
//! at compile time picks up `0033_scheduled_messages.sql` automatically;
//! no per-file migration list to keep in sync).

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use chrono::{Duration, Utc};
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-sched-routes-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
    });
}

struct TestApp {
    app: Router,
    chat: SqlitePool,
    auth: SqlitePool,
    alice_id: String,
    alice_session: String,
    bob_id: String,
    bob_session: String,
}

async fn fresh_app() -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let alice_id = db::auth::create_user(&auth, "alice", "hash").await.unwrap();
    let bob_id = db::auth::create_user(&auth, "bob", "hash").await.unwrap();
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
        .bind(&alice_id)
        .execute(&auth)
        .await
        .unwrap();
    let alice_session = db::auth::create_session(&auth, &alice_id).await.unwrap();
    let bob_session = db::auth::create_session(&auth, &bob_id).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth: auth.clone(),
        chat: chat.clone(),
        settings,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: None,
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
    };
    let app = routes::build_router(state);
    TestApp {
        app,
        chat,
        auth,
        alice_id,
        alice_session,
        bob_id,
        bob_session,
    }
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().copied())
        .finish()
}

async fn send(
    app: &Router,
    sess: &str,
    method: Method,
    uri: &str,
    body: Option<String>,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"));
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    let req = builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 4 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn iso(dt: chrono::DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

async fn first_pending_id(chat: &SqlitePool, user_id: &str) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT id FROM scheduled_messages WHERE user_id = ? AND delivered_at IS NULL \
         ORDER BY id ASC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(chat)
    .await
    .unwrap()
}

#[tokio::test]
async fn post_creates_pending_row_for_valid_input() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "hello future"),
        ("scheduled_for", &when),
    ]);

    let (status, resp) = send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {resp}");

    let id = first_pending_id(&t.chat, &t.alice_id)
        .await
        .expect("pending row inserted");
    let row = db::scheduled::get_scheduled(&t.chat, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.room_id, 1);
    assert_eq!(row.body, "hello future");
    assert!(row.delivered_at.is_none());
    assert_eq!(row.repeat, "none", "absent repeat defaults to one-shot");
}

// LC-485: the `repeat` form field is stored on the pending row.
#[tokio::test]
async fn post_with_repeat_stores_recurrence() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "daily standup"),
        ("scheduled_for", &when),
        ("repeat", "daily"),
    ]);
    let (status, resp) = send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {resp}");

    let id = first_pending_id(&t.chat, &t.alice_id)
        .await
        .expect("pending row");
    let row = db::scheduled::get_scheduled(&t.chat, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.repeat, "daily");
}

// LC-485: an unknown repeat value is rejected.
#[tokio::test]
async fn post_rejects_invalid_repeat() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "bad"),
        ("scheduled_for", &when),
        ("repeat", "hourly"),
    ]);
    let (status, _resp) = send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_rejects_over_length_body() {
    // LC-349: the scheduled body must be length-capped on the write path like the
    // live send path (MAX_MESSAGE_CHARS = 16_000), or it renders uncapped through
    // markdown::render on /scheduled views and at delivery (LC-153 hazard).
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let huge = "a".repeat(16_001);
    let body = form_body(&[("room_id", "1"), ("body", &huge), ("scheduled_for", &when)]);

    let (status, resp) = send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "over-length body must be rejected, got: {resp}"
    );
    assert!(
        first_pending_id(&t.chat, &t.alice_id).await.is_none(),
        "no scheduled row should be inserted for an over-length body"
    );
}

#[tokio::test]
async fn post_rejects_lead_time_too_short() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::seconds(10));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "too soon"),
        ("scheduled_for", &when),
    ]);
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(first_pending_id(&t.chat, &t.alice_id).await.is_none());
}

#[tokio::test]
async fn post_rejects_lead_time_too_long() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::days(400));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "too far"),
        ("scheduled_for", &when),
    ]);
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(first_pending_id(&t.chat, &t.alice_id).await.is_none());
}

#[tokio::test]
async fn post_denies_user_without_room_access() {
    let t = fresh_app().await;
    // Create a private room without bob as a member.
    let general_id: i64 = sqlx::query_scalar("SELECT id FROM enclaves WHERE name = 'General'")
        .fetch_one(&t.chat)
        .await
        .unwrap();
    let private_id = db::chat::create_room(
        &t.chat,
        "secret",
        None,
        "private",
        Some("code-abc"),
        Some(general_id),
    )
    .await
    .unwrap();
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[
        ("room_id", &private_id.to_string()),
        ("body", "sneaking in"),
        ("scheduled_for", &when),
    ]);
    let (status, _) = send(
        &t.app,
        &t.bob_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(first_pending_id(&t.chat, &t.bob_id).await.is_none());
}

#[tokio::test]
async fn post_denies_attachment_owned_by_another_user() {
    let t = fresh_app().await;
    // Alice uploads. Bob then tries to schedule a message attaching alice's file.
    let upload_id = db::uploads::insert_upload(
        &t.chat,
        &t.alice_id,
        "a.png",
        "image/png",
        10,
        "alice-image.png",
        None,
    )
    .await
    .unwrap();
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "stolen attachment"),
        ("scheduled_for", &when),
        ("file_id", &upload_id.to_string()),
    ]);
    let (status, _) = send(
        &t.app,
        &t.bob_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(first_pending_id(&t.chat, &t.bob_id).await.is_none());
}

#[tokio::test]
async fn patch_own_pending_returns_updated_row_fragment() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[("room_id", "1"), ("body", "v1"), ("scheduled_for", &when)]);
    send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    let id = first_pending_id(&t.chat, &t.alice_id).await.unwrap();

    let when2 = iso(Utc::now() + Duration::hours(2));
    let patch_body = form_body(&[("body", "v2 updated"), ("scheduled_for", &when2)]);
    let (status, resp) = send(
        &t.app,
        &t.alice_session,
        Method::PATCH,
        &format!("/scheduled/{id}"),
        Some(patch_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resp: {resp}");
    assert!(
        resp.contains(&format!("id=\"scheduled-row-{id}\"")),
        "fragment must carry the row id; got: {resp}"
    );
    let row = db::scheduled::get_scheduled(&t.chat, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.body, "v2 updated");
}

#[tokio::test]
async fn patch_other_users_row_returns_403() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "alice's row"),
        ("scheduled_for", &when),
    ]);
    send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    let id = first_pending_id(&t.chat, &t.alice_id).await.unwrap();

    let when2 = iso(Utc::now() + Duration::hours(2));
    let patch_body = form_body(&[("body", "hijacked"), ("scheduled_for", &when2)]);
    let (status, _) = send(
        &t.app,
        &t.bob_session,
        Method::PATCH,
        &format!("/scheduled/{id}"),
        Some(patch_body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let row = db::scheduled::get_scheduled(&t.chat, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.body, "alice's row",
        "wrong-user PATCH must not mutate row"
    );
}

#[tokio::test]
async fn patch_delivered_row_returns_410() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let create = form_body(&[
        ("room_id", "1"),
        ("body", "to be delivered"),
        ("scheduled_for", &when),
    ]);
    send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(create),
    )
    .await;
    let id = first_pending_id(&t.chat, &t.alice_id).await.unwrap();

    // Simulate delivery: mark row as delivered.
    sqlx::query("UPDATE scheduled_messages SET delivered_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(&t.chat)
        .await
        .unwrap();

    let when2 = iso(Utc::now() + Duration::hours(2));
    let patch_body = form_body(&[("body", "edit after delivery"), ("scheduled_for", &when2)]);
    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::PATCH,
        &format!("/scheduled/{id}"),
        Some(patch_body),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
}

#[tokio::test]
async fn delete_own_pending_returns_200_and_removes_row() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "cancel me"),
        ("scheduled_for", &when),
    ]);
    send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    let id = first_pending_id(&t.chat, &t.alice_id).await.unwrap();

    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::DELETE,
        &format!("/scheduled/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(db::scheduled::get_scheduled(&t.chat, id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_other_users_row_returns_403() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "alice's row"),
        ("scheduled_for", &when),
    ]);
    send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    let id = first_pending_id(&t.chat, &t.alice_id).await.unwrap();

    let (status, _) = send(
        &t.app,
        &t.bob_session,
        Method::DELETE,
        &format!("/scheduled/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(db::scheduled::get_scheduled(&t.chat, id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn delete_delivered_row_returns_410() {
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    let body = form_body(&[
        ("room_id", "1"),
        ("body", "to deliver"),
        ("scheduled_for", &when),
    ]);
    send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(body),
    )
    .await;
    let id = first_pending_id(&t.chat, &t.alice_id).await.unwrap();
    sqlx::query("UPDATE scheduled_messages SET delivered_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(&t.chat)
        .await
        .unwrap();

    let (status, _) = send(
        &t.app,
        &t.alice_session,
        Method::DELETE,
        &format!("/scheduled/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
}

#[tokio::test]
async fn get_lists_only_caller_pending_rows_in_order() {
    let t = fresh_app().await;
    // Alice schedules two; Bob schedules one. Alice's GET sees only hers.
    let later = iso(Utc::now() + Duration::hours(2));
    let sooner = iso(Utc::now() + Duration::hours(1));
    send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(form_body(&[
            ("room_id", "1"),
            ("body", "alice later"),
            ("scheduled_for", &later),
        ])),
    )
    .await;
    send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(form_body(&[
            ("room_id", "1"),
            ("body", "alice sooner"),
            ("scheduled_for", &sooner),
        ])),
    )
    .await;
    send(
        &t.app,
        &t.bob_session,
        Method::POST,
        "/scheduled",
        Some(form_body(&[
            ("room_id", "1"),
            ("body", "bob hidden"),
            ("scheduled_for", &sooner),
        ])),
    )
    .await;

    let (status, html) = send(&t.app, &t.alice_session, Method::GET, "/scheduled", None).await;
    assert_eq!(status, StatusCode::OK);
    let sooner_pos = html.find("alice sooner").expect("sooner row present");
    let later_pos = html.find("alice later").expect("later row present");
    assert!(
        sooner_pos < later_pos,
        "sooner row must render before later row"
    );
    assert!(
        !html.contains("bob hidden"),
        "other-user row must not appear"
    );
}

#[tokio::test]
async fn get_renders_renamed_mention_as_literal_text_not_chip() {
    // Option B visible signal: a scheduled message body containing @bob
    // is rendered against the current users table on every GET. After
    // bob is renamed to robert, the GET /scheduled page shows literal
    // "@bob" text with no profile-link chip; the author has a visible
    // signal that the mention will silently drop at delivery.
    let t = fresh_app().await;
    let when = iso(Utc::now() + Duration::hours(1));
    send(
        &t.app,
        &t.alice_session,
        Method::POST,
        "/scheduled",
        Some(form_body(&[
            ("room_id", "1"),
            ("body", "ping @bob"),
            ("scheduled_for", &when),
        ])),
    )
    .await;

    // Pre-rename: chip should resolve (anchor with bob's profile link).
    let (_, pre_html) = send(&t.app, &t.alice_session, Method::GET, "/scheduled", None).await;
    let bob_profile = format!("/profile/{}", t.bob_id);
    assert!(
        pre_html.contains(&bob_profile),
        "pre-rename: @bob should render as a profile-link chip"
    );

    // Rename bob away; the @bob token no longer resolves.
    sqlx::query("UPDATE users SET username = 'robert' WHERE id = ?")
        .bind(&t.bob_id)
        .execute(&t.auth)
        .await
        .unwrap();

    let (_, post_html) = send(&t.app, &t.alice_session, Method::GET, "/scheduled", None).await;
    assert!(
        !post_html.contains(&bob_profile),
        "post-rename: chip anchor for bob's id must be absent"
    );
    assert!(
        post_html.contains("@bob"),
        "post-rename: literal @bob text must survive in the body"
    );
}
