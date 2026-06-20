//! LC-393: call-transcription endpoints (Phase 1, DM calls). Covers the access
//! gate, segment persistence + length cap, the idempotent /end + "transcript
//! saved" notice, and the gated saved-transcript page. Mirrors the
//! routes_reactions_authz harness (admin promote + General backfill).

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-transcripts-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

mod common;

async fn post(app: &Router, sess: &str, uri: &str, body: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"));
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    let req = req
        .body(Body::from(body.unwrap_or("").to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

struct Setup {
    app: Router,
    chat: sqlx::SqlitePool,
    hub: Arc<Hub>,
    a_session: String,
    b_session: String,
    b_id: String,
    outsider_session: String,
    dm_room: i64,
    public_room: i64,
    voice_room: i64,
}

async fn setup() -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let a = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let b = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    let outsider = db::auth::create_user(&auth, "carol", "h").await.unwrap();
    // Promote one user so backfill_general_membership runs (harness gotcha).
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&a)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    // A DM room with alice + bob; carol is not a member.
    let dm_room = db::chat::create_room(&chat, "alice-bob", None, "dm", None, None)
        .await
        .unwrap();
    db::chat::add_room_member(&chat, dm_room, &a).await.unwrap();
    db::chat::add_room_member(&chat, dm_room, &b).await.unwrap();

    // A non-DM room alice belongs to (General = room 1 from backfill is public).
    let public_room = 1;

    // A public voice channel (Phase 2). is_voice gates it as call-capable;
    // participation is tracked by the hub, not room_members.
    let voice_room = db::chat::create_room(&chat, "voicechan", None, "public", None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE rooms SET is_voice = 1 WHERE id = ?")
        .bind(voice_room)
        .execute(&chat)
        .await
        .unwrap();

    let a_session = db::auth::create_session(&auth, &a).await.unwrap();
    let b_session = db::auth::create_session(&auth, &b).await.unwrap();
    let outsider_session = db::auth::create_session(&auth, &outsider).await.unwrap();

    let hub = Arc::new(Hub::new());
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        auth,
        chat: chat.clone(),
        settings,
        hub: hub.clone(),
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
    };
    Setup {
        app: routes::build_router(state),
        chat,
        hub,
        a_session,
        b_session,
        b_id: b,
        outsider_session,
        dm_room,
        public_room,
        voice_room,
    }
}

/// Pull the transcript_id out of the start response JSON.
fn parse_id(body: &str) -> i64 {
    let v: serde_json::Value = serde_json::from_str(body).expect("json");
    v["transcript_id"].as_i64().expect("transcript_id")
}

#[tokio::test]
async fn member_starts_session_outsider_forbidden_nondm_404() {
    let s = setup().await;

    let (st, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "member can start: {body}");
    let tid = parse_id(&body);
    assert!(tid > 0);

    // A second member toggling joins the SAME open session, not a new one.
    let (st2, body2) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    assert_eq!(st2, StatusCode::OK);
    assert_eq!(parse_id(&body2), tid, "second start joins the open session");

    // Outsider cannot start on a DM they're not in.
    let (st3, _) = post(
        &s.app,
        &s.outsider_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    assert_eq!(st3, StatusCode::FORBIDDEN);

    // Transcription is DM-only: a non-DM room 404s.
    let (st4, _) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.public_room),
        None,
    )
    .await;
    assert_eq!(st4, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn voice_channel_participant_transcribes_nonparticipant_forbidden() {
    let s = setup().await;

    // A voice channel is call-capable (passes the room gate), but a user who is
    // not currently joined to the channel cannot start - so a room member can't
    // silently auto-capture everyone who did join.
    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.voice_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "non-participant cannot start");

    // Register bob as a live participant in the channel (what the WS voice_join
    // does), then he can start + post.
    let (conn, _rx, _) = s.hub.connect(&s.b_id, "bob");
    s.hub.voice_join(conn, s.voice_room);

    let (st, body) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.voice_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "joined participant starts: {body}");
    let tid = parse_id(&body);

    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=voice+hello"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].text, "voice hello");

    // Carol, not a channel participant, cannot post a segment.
    let (st, _) = post(
        &s.app,
        &s.outsider_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=nope"),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // A participant ends the session for the whole channel; transcript saved.
    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/end"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let t = db::transcripts::get(&s.chat, tid).await.unwrap().unwrap();
    assert_eq!(t.status, "ended");
}

#[tokio::test]
async fn segments_persist_and_render_gated() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);

    // Each party posts a segment (each transcribes its own mic).
    let (st, _) = post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=hello+there"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=general+kenobi"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Outsider cannot post a segment.
    let (st, _) = post(
        &s.app,
        &s.outsider_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=intruder"),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Both segments persisted, in order.
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].text, "hello there");
    assert_eq!(segs[1].text, "general kenobi");

    // The saved-transcript page renders the lines to a member, 403s an outsider.
    let (st, page) = get(&s.app, &s.b_session, &format!("/transcripts/{tid}")).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        page.contains("hello there"),
        "page shows the captions: {page}"
    );
    assert!(page.contains("general kenobi"));
    let (st, _) = get(&s.app, &s.outsider_session, &format!("/transcripts/{tid}")).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn segment_text_is_length_capped() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);

    let long = "a".repeat(db::transcripts::MAX_SEGMENT_CHARS + 500);
    let (st, _) = post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some(&format!("text={long}")),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!(
        segs[0].text.chars().count(),
        db::transcripts::MAX_SEGMENT_CHARS,
        "stored text is capped at MAX_SEGMENT_CHARS"
    );
}

#[tokio::test]
async fn end_is_idempotent_and_posts_one_saved_notice() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);

    // End twice; only the first transition posts the saved notice.
    for _ in 0..2 {
        let (st, _) = post(
            &s.app,
            &s.a_session,
            &format!("/call/transcript/{tid}/end"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }

    // Session is ended.
    let t = db::transcripts::get(&s.chat, tid).await.unwrap().unwrap();
    assert_eq!(t.status, "ended");
    assert!(t.ended_at.is_some());

    // Exactly one "transcript saved" system message landed in the DM.
    let msgs = db::chat::list_messages(&s.chat, s.dm_room).await.unwrap();
    let saved = msgs
        .iter()
        .filter(|m| m.body.contains(&format!("/transcripts/{tid}")))
        .count();
    assert_eq!(saved, 1, "the saved notice is posted exactly once");

    // A late segment after the session closed is dropped, not an error.
    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=too+late"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert!(segs.is_empty(), "late segment dropped after end");
}
