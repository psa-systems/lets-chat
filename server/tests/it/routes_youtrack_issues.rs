//! LC-1135: gates for turning transcript action items into YouTrack issues.
//! The success path of the YouTrack HTTP call is covered by `youtrack`'s unit
//! tests; here we pin the route gates (flag off, non-staff file) and that a
//! generation with the flag + AI on stores and renders proposals.

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, models::enclave::EnclaveRole, routes, state::AppState, ws::hub::Hub};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

use crate::common;

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-youtrack-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct TestApp {
    app: Router,
    owner: String,
    owner_session: String,
    member: String,
    member_session: String,
    chat: SqlitePool,
    settings: SqlitePool,
}

async fn app(canned_llm: Option<&str>) -> TestApp {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    let owner = db::auth::create_user(&auth, "owner", "h").await.unwrap();
    let member = db::auth::create_user(&auth, "member", "h").await.unwrap();
    for id in [&owner, &member] {
        sqlx::query("UPDATE users SET totp_enabled=1 WHERE id=?")
            .bind(id)
            .execute(&auth)
            .await
            .unwrap();
    }
    let owner_session = db::auth::create_session(&auth, &owner).await.unwrap();
    let member_session = db::auth::create_session(&auth, &member).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let bg = lets_chat::bg::spawn(auth.clone());
    let llm_client: Option<Arc<dyn lets_chat::llm::LlmClient>> = canned_llm.map(|c| {
        Arc::new(lets_chat::llm::MockLlmClient {
            canned: c.to_string(),
        }) as Arc<dyn lets_chat::llm::LlmClient>
    });
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth: auth.clone(),
        chat: chat.clone(),
        settings: settings.clone(),
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
        llm_client,
        embedding_client: None,
    };
    TestApp {
        app: routes::build_router(state),
        owner,
        owner_session,
        member,
        member_session,
        chat,
        settings,
    }
}

async fn post(app: &Router, sess: &str, uri: &str, body: &str) -> (StatusCode, String) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::COOKIE, format!("session={sess}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Public room, owner owns the enclave, member is a plain member. Returns an
/// open transcript id with one spoken segment.
async fn seed_transcript(t: &TestApp) -> i64 {
    let eid = db::enclave::create_enclave(&t.chat, "Acme", None, &t.owner)
        .await
        .unwrap();
    db::enclave::add_member(&t.chat, eid, &t.member, EnclaveRole::Member)
        .await
        .unwrap();
    let room = db::chat::create_room(&t.chat, "general", None, "public", None, Some(eid))
        .await
        .unwrap();
    let (session, _) = db::transcripts::start_session(&t.chat, room, &t.owner)
        .await
        .unwrap();
    db::transcripts::append_segment(
        &t.chat,
        session.id,
        &t.owner,
        "I will ship the thing",
        None,
        0,
    )
    .await
    .unwrap();
    session.id
}

async fn enable_ai_and_flag(t: &TestApp, transcript_id: i64) {
    // YouTrack + AI flags on, AI audience everyone (default), room AI on.
    db::settings::set_setting(&t.settings, "youtrack_issues_enabled", "true")
        .await
        .unwrap();
    db::settings::set_setting(&t.settings, "llm_enabled", "true")
        .await
        .unwrap();
    let room_id = db::transcripts::get(&t.chat, transcript_id)
        .await
        .unwrap()
        .unwrap()
        .room_id;
    db::chat::set_room_assistant_enabled(&t.chat, room_id, true)
        .await
        .unwrap();
}

#[tokio::test]
async fn propose_issues_is_forbidden_when_flag_off() {
    let t = app(Some("[]")).await;
    let tid = seed_transcript(&t).await;
    // Flag is off by default.
    let (status, _) = post(
        &t.app,
        &t.owner_session,
        &format!("/transcripts/{tid}/propose-issues"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn propose_issues_stores_and_renders_with_flag_and_ai_on() {
    let canned = r#"[{"summary":"Ship the thing","assignee":"owner","source_lines":"owner: I will ship the thing"},
                     {"summary":"Write the doc","assignee":null,"source_lines":"owner: and document it"}]"#;
    let t = app(Some(canned)).await;
    let tid = seed_transcript(&t).await;
    enable_ai_and_flag(&t, tid).await;

    let (status, body) = post(
        &t.app,
        &t.owner_session,
        &format!("/transcripts/{tid}/propose-issues"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("Ship the thing"), "renders summary: {body}");
    assert!(body.contains("Write the doc"));
    let rows = db::transcript_proposals::list_for_transcript(&t.chat, tid)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].assignee_name.as_deref(), Some("owner"));
}

#[tokio::test]
async fn file_issues_is_forbidden_for_a_non_staff_member() {
    let t = app(None).await;
    let tid = seed_transcript(&t).await;
    db::settings::set_setting(&t.settings, "youtrack_issues_enabled", "true")
        .await
        .unwrap();
    // Member is a plain enclave member, not staff -> 403 at the file gate,
    // before any YouTrack call.
    let (status, _) = post(
        &t.app,
        &t.member_session,
        &format!("/transcripts/{tid}/file-issues"),
        "accept_1=on",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn file_issues_passes_the_staff_gate_then_400s_when_unconfigured() {
    let t = app(None).await;
    let tid = seed_transcript(&t).await;
    db::settings::set_setting(&t.settings, "youtrack_issues_enabled", "true")
        .await
        .unwrap();
    // Owner is enclave Owner (staff), so the staff gate passes; with no YouTrack
    // connection configured the route then fails with 400, not 403. This proves
    // can_file admits staff.
    let (status, _) = post(
        &t.app,
        &t.owner_session,
        &format!("/transcripts/{tid}/file-issues"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
