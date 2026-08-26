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

async fn post_raw(
    app: &Router,
    sess: &str,
    uri: &str,
    body: &[u8],
    content_type: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// LC-813: POST a server-capture clip as the transcription agent would - no
/// session cookie, a bearer token, and an explicit `X-Speaker-Id`.
async fn post_agent(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    speaker_id: Option<&str>,
    body: &[u8],
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "audio/webm");
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(s) = speaker_id {
        req = req.header("X-Speaker-Id", s);
    }
    let req = req.body(Body::from(body.to_vec())).unwrap();
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
    state: AppState,
    chat: sqlx::SqlitePool,
    hub: Arc<Hub>,
    a_session: String,
    b_session: String,
    b_id: String,
    outsider_session: String,
    /// LC-813: carol's user id, for asserting the agent-clip route rejects a
    /// speaker who exists but is not a live participant of the call.
    outsider_id: String,
    dm_room: i64,
    public_room: i64,
    voice_room: i64,
    /// LC-597: a stage-enabled public room. Stage audio rides the LiveKit SFU,
    /// so participation lives in `Hub::stages`, not `voice_rooms`.
    stage_room: i64,
}

async fn setup() -> Setup {
    setup_with_clients(None, None).await
}

async fn setup_with_stt(stt_client: Option<Arc<dyn lets_chat::stt::SttClient>>) -> Setup {
    setup_with_clients(stt_client, None).await
}

async fn setup_with_clients(
    stt_client: Option<Arc<dyn lets_chat::stt::SttClient>>,
    llm_client: Option<Arc<dyn lets_chat::llm::LlmClient>>,
) -> Setup {
    // LC-619: the /audio route sheds a clip with 429 from two independent STT
    // load controls, both process-wide and both tripped by the parallel
    // transcripts binary submitting clips across concurrent STT tests, which
    // intermittently failed an unrelated test:
    //   1. try_admit's per-minute rate cap (SttGlobal 30/min, SttRoom 10/min by
    //      default), read from the environment on each call.
    //   2. the concurrency semaphore (LETS_CHAT_STT_WORKERS permits, default 2),
    //      a OnceLock read once at the first transcription.
    // No test asserts either limit, so raise all three out of the way. WORKERS
    // must be set before the first /audio POST anywhere; every test runs this
    // builder before posting, so the OnceLock initializes to the high value.
    std::env::set_var("LETS_CHAT_STT_RATE_GLOBAL", "1000000");
    std::env::set_var("LETS_CHAT_STT_RATE_ROOM", "1000000");
    std::env::set_var("LETS_CHAT_STT_WORKERS", "1024");
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;
    // LC-679: opt this AI-feature test into the runtime flag (default off).
    db::settings::set_setting(&settings, "llm_enabled", "true")
        .await
        .unwrap();

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

    // LC-597: a stage-enabled public room. `stage_enabled` is a rooms column;
    // who is on the stage (and who holds the floor) is hub state.
    let stage_room = db::chat::create_room(&chat, "stagechan", None, "public", None, None)
        .await
        .unwrap();
    db::chat::set_room_stage_enabled(&chat, stage_room, true)
        .await
        .unwrap();

    let a_session = db::auth::create_session(&auth, &a).await.unwrap();
    let b_session = db::auth::create_session(&auth, &b).await.unwrap();
    let outsider_session = db::auth::create_session(&auth, &outsider).await.unwrap();

    let hub = Arc::new(Hub::new());
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
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
        stt_client,
        llm_client,
        embedding_client: None,
    };
    Setup {
        app: routes::build_router(state.clone()),
        state,
        chat,
        hub,
        a_session,
        b_session,
        b_id: b,
        outsider_session,
        outsider_id: outsider,
        dm_room,
        public_room,
        voice_room,
        stage_room,
    }
}

/// Pull the transcript_id out of the start response JSON.
fn parse_id(body: &str) -> i64 {
    let v: serde_json::Value = serde_json::from_str(body).expect("json");
    v["transcript_id"].as_i64().expect("transcript_id")
}

#[tokio::test]
async fn member_starts_session_outsider_and_non_huddle_participant_forbidden() {
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

    // LC-553: a group room is huddle-capable (a huddle is the same mesh as a
    // voice channel), so it is no longer "not a call surface" and no longer
    // 404s. Authority now comes from live mesh membership instead: a room member
    // who is not in the huddle is FORBIDDEN rather than hidden.
    let (st4, _) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.public_room),
        None,
    )
    .await;
    assert_eq!(st4, StatusCode::FORBIDDEN);
}

/// A late joiner learns of an already-open session via GET .../transcript/active
/// so it can adopt it and auto-start its own capture. Null before any session,
/// the open id after, and non-participants are forbidden.
#[tokio::test]
async fn active_reports_the_open_session_to_a_participant() {
    let s = setup().await;

    // Nothing open yet -> null.
    let (st, body) = get(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/active", s.dm_room),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "member may query active: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(v["transcript_id"].is_null(), "no session open yet: {body}");

    // One member starts; the other sees the same session id.
    let (st, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let tid = parse_id(&body);

    let (st, body) = get(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/active", s.dm_room),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        v["transcript_id"].as_i64(),
        Some(tid),
        "late joiner sees the open session: {body}"
    );

    // A non-participant may not probe the call.
    let (st, _) = get(
        &s.app,
        &s.outsider_session,
        &format!("/call/{}/transcript/active", s.dm_room),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "non-participant is forbidden");
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

/// LC-597: the Stage is the one real-time surface transcription never reached.
/// Its audio rides the LiveKit SFU rather than the WebRTC mesh, so a speaker is
/// absent from `voice_room_users` and `require_participant` refused them.
///
/// Only the floor is captured. A listener on a Stage is audience - they publish
/// no track - so capturing them would record a room, not a call.
#[tokio::test]
async fn stage_speaker_transcribes_listener_forbidden() {
    let s = setup().await;

    // On the stage as a LISTENER: present in the roster, no floor.
    s.hub.stage_join(s.stage_room, &s.b_id);
    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.stage_room),
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a listener holds no floor and cannot start"
    );

    // Granted the floor (what a host's stage_promote does).
    s.hub.stage_promote(s.stage_room, &s.b_id);
    let (st, body) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.stage_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "speaker starts: {body}");
    let tid = parse_id(&body);

    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=stage+hello"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 1, "segment persists for a stage room");
    assert_eq!(segs[0].text, "stage hello");

    // Carol is not on the stage at all.
    let (st, _) = post(
        &s.app,
        &s.outsider_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=nope"),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Stepping down revokes it immediately: the floor is the gate, not the
    // session. This is the server half of the hot-mic stop in transcribe.js.
    s.hub.stage_demote(s.stage_room, &s.b_id);
    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=after+step+down"),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a demoted speaker cannot keep feeding the session"
    );
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 1, "the refused segment was not stored");
}

/// LC-597: the hub roster is not cleared when a host turns the stage off in room
/// settings, so the floor has to be re-checked against the room rather than
/// trusted from the roster alone. Without the `stage_enabled` re-check, everyone
/// holding the floor at the moment it was disabled keeps their grant.
#[tokio::test]
async fn disabling_the_stage_revokes_the_floor() {
    let s = setup().await;

    s.hub.stage_join(s.stage_room, &s.b_id);
    s.hub.stage_promote(s.stage_room, &s.b_id);
    let (st, body) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.stage_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "speaker starts while the stage is on");
    let tid = parse_id(&body);

    db::chat::set_room_stage_enabled(&s.chat, s.stage_room, false)
        .await
        .unwrap();
    // The roster is untouched - bob is still listed as holding the floor.
    assert!(
        s.hub
            .stage_roster(s.stage_room)
            .is_some_and(|r| r.speakers.contains(&s.b_id)),
        "precondition: the hub still lists bob as a speaker"
    );

    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=stage+is+off"),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a stale roster entry must not outlive the stage"
    );
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

/// LC-791: an ad-hoc huddle in a room with NO per-room `room_members` rows (an
/// enclave text room, where access is enclave-scoped) still broadcasts live
/// captions to its mesh participants. The recipient set was `list_room_member_ids`
/// for non-voice rooms, which is empty here, so a participant's own caption was
/// stored but broadcast to no one and never appeared live (the saved transcript
/// still had it). Recipients now union in `voice_room_users`.
#[tokio::test]
async fn huddle_without_room_members_broadcasts_captions_to_participants() {
    let s = setup().await;
    // A text room created without any room_members rows (mirrors an enclave
    // channel). Bob is NOT a member of it.
    let room = db::chat::create_room(&s.chat, "huddle", None, "public", None, None)
        .await
        .unwrap();
    // Bob joins the huddle mesh - which is what require_participant admits - but
    // holds no room_members row.
    let (conn, mut rx, _) = s.hub.connect(&s.b_id, "bob");
    s.hub.voice_join(conn, room);

    let (st, body) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{room}/transcript/start"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "start: {body}");
    let tid = parse_id(&body);

    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=hello+there"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Bob must receive his own caption over the hub, addressed to him. Before the
    // fix the recipient set was empty, so nothing was broadcast.
    let mut got = false;
    while let Ok(ev) = rx.try_recv() {
        if let lets_chat::ws::events::ChatEvent::TranscriptSegment {
            to_user_id, text, ..
        } = ev
        {
            if to_user_id == s.b_id && text == "hello there" {
                got = true;
                break;
            }
        }
    }
    assert!(
        got,
        "a huddle participant should receive their own live caption even when the room has no room_members rows"
    );
}

#[tokio::test]
async fn server_stt_audio_records_segment() {
    // Phase 3: with a server STT client configured, an audio clip POSTed to
    // /audio is transcribed and stored as a segment (here via a mock engine).
    let mock: Arc<dyn lets_chat::stt::SttClient> =
        Arc::new(lets_chat::stt::MockSttClient::text("mock transcription"));
    let s = setup_with_stt(Some(mock)).await;

    // Bob joins the voice channel and opens a session.
    let (conn, _rx, _) = s.hub.connect(&s.b_id, "bob");
    s.hub.voice_join(conn, s.voice_room);
    let (_, body) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.voice_room),
        None,
    )
    .await;
    let tid = parse_id(&body);

    // POST an audio clip; the mock engine returns the canned text, stored as a
    // segment attributed to bob.
    let (st, _) = post_raw(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/audio"),
        b"fake-opus-bytes",
        "audio/webm",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].text, "mock transcription");
    assert_eq!(segs[0].user_id, s.b_id);
    // Plain-text mock (no segments) -> unknown duration, so the export stays
    // synthetic for this one.
    assert_eq!(segs[0].duration_ms, 0);

    // A non-participant still cannot push audio.
    let (st, _) = post_raw(
        &s.app,
        &s.outsider_session,
        &format!("/call/transcript/{tid}/audio"),
        b"x",
        "audio/webm",
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

/// LC-813 (LC-810 stage 1): the shared agent-clip token for these tests. Set on
/// the process env (the route reads it there); every agent test writes the same
/// value, so concurrent writes in the parallel binary are benign.
const AGENT_TOKEN: &str = "test-agent-secret";

async fn open_voice_session(s: &Setup) -> i64 {
    let (conn, _rx, _) = s.hub.connect(&s.b_id, "bob");
    s.hub.voice_join(conn, s.voice_room);
    let (_, body) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.voice_room),
        None,
    )
    .await;
    parse_id(&body)
}

#[tokio::test]
async fn agent_clip_attributes_segment_to_named_speaker() {
    // The server-capture path posts a clip attributed to the track's participant
    // (bob), with NO caller session - the segment is bob's, not the poster's.
    std::env::set_var("LETS_CHAT_TRANSCRIBE_AGENT_TOKEN", AGENT_TOKEN);
    let mock: Arc<dyn lets_chat::stt::SttClient> =
        Arc::new(lets_chat::stt::MockSttClient::text("agent heard bob"));
    let s = setup_with_stt(Some(mock)).await;
    let tid = open_voice_session(&s).await;

    let (st, _) = post_agent(
        &s.app,
        &format!("/call/transcript/{tid}/agent-clip"),
        Some(AGENT_TOKEN),
        Some(&s.b_id),
        b"fake-track-audio",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].text, "agent heard bob");
    assert_eq!(
        segs[0].user_id, s.b_id,
        "attributed to the named speaker, not the caller"
    );
}

#[tokio::test]
async fn agent_clip_rejects_missing_or_wrong_token() {
    std::env::set_var("LETS_CHAT_TRANSCRIBE_AGENT_TOKEN", AGENT_TOKEN);
    let mock: Arc<dyn lets_chat::stt::SttClient> =
        Arc::new(lets_chat::stt::MockSttClient::text("nope"));
    let s = setup_with_stt(Some(mock)).await;
    let tid = open_voice_session(&s).await;
    let uri = format!("/call/transcript/{tid}/agent-clip");

    let (st, _) = post_agent(&s.app, &uri, Some("wrong-secret"), Some(&s.b_id), b"a").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "wrong token rejected");
    let (st, _) = post_agent(&s.app, &uri, None, Some(&s.b_id), b"a").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "no token rejected");
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert!(segs.is_empty(), "no segment stored for a rejected token");
}

#[tokio::test]
async fn agent_clip_rejects_speaker_outside_the_room() {
    // Defense in depth: even with a valid token, the named speaker must be a live
    // participant of THIS call, so a leaked token cannot forge an arbitrary user.
    std::env::set_var("LETS_CHAT_TRANSCRIBE_AGENT_TOKEN", AGENT_TOKEN);
    let mock: Arc<dyn lets_chat::stt::SttClient> =
        Arc::new(lets_chat::stt::MockSttClient::text("forged"));
    let s = setup_with_stt(Some(mock)).await;
    let tid = open_voice_session(&s).await;

    // Carol exists but never joined the voice channel.
    let (st, _) = post_agent(
        &s.app,
        &format!("/call/transcript/{tid}/agent-clip"),
        Some(AGENT_TOKEN),
        Some(&s.outsider_id),
        b"forged-audio",
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert!(segs.is_empty(), "no segment stored for a non-participant");
}

#[tokio::test]
async fn audio_rejected_when_server_stt_disabled() {
    // Default setup has no STT client: /audio is 400 (clients only POST audio
    // when /call/config reports sttServer=true).
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    let (st, _) = post_raw(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/audio"),
        b"audio",
        "audio/webm",
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn archive_lists_only_accessible_transcripts() {
    let s = setup().await;
    // alice opens + ends a transcript in the alice-bob DM.
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/end"),
        None,
    )
    .await;

    // A member sees her transcript in the archive (links to the full view).
    let (st, page) = get(&s.app, &s.a_session, "/transcripts").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        page.contains(&format!("/transcripts/{tid}")),
        "member sees her transcript in the archive"
    );

    // A non-member's archive never lists it.
    let (st, page) = get(&s.app, &s.outsider_session, "/transcripts").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        !page.contains(&format!("/transcripts/{tid}")),
        "non-member must not see another DM's transcript in the archive"
    );
}

#[tokio::test]
async fn summary_generates_stores_and_gates() {
    let mock: Arc<dyn lets_chat::llm::LlmClient> = Arc::new(lets_chat::llm::MockLlmClient {
        canned: "## Summary\nThey discussed the roadmap.\n\n## Action items\n- Ship it".to_string(),
    });
    let s = setup_with_clients(None, Some(mock)).await;

    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=lets+talk+roadmap"),
    )
    .await;
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/end"),
        None,
    )
    .await;

    // Member generates the summary; the rendered markdown comes back + is stored.
    let (st, frag) = post(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/summary"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(frag.contains("Action items"), "summary fragment: {frag}");
    let t = db::transcripts::get(&s.chat, tid).await.unwrap().unwrap();
    assert!(t.summary.unwrap().contains("roadmap"), "summary persisted");

    // The transcript page now shows the stored summary.
    let (st, page) = get(&s.app, &s.a_session, &format!("/transcripts/{tid}")).await;
    assert_eq!(st, StatusCode::OK);
    assert!(page.contains("They discussed the roadmap"));

    // Non-member cannot summarize.
    let (st, _) = post(
        &s.app,
        &s.outsider_session,
        &format!("/transcripts/{tid}/summary"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn summary_rejected_when_llm_disabled() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    let (st, _) = post(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/summary"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// LC-591: a verbose_json engine returns real segment timings; the stored
/// segment carries the real spoken duration, and the WebVTT export uses it for
/// the cue length instead of the synthetic "until the next cue" fallback.
#[tokio::test]
async fn server_stt_stores_real_segment_duration_and_exports_it() {
    use lets_chat::stt::{MockSttClient, SttSegment};
    // Segments span 0.0..2.5s -> duration 2500ms.
    let mock: Arc<dyn lets_chat::stt::SttClient> = Arc::new(MockSttClient {
        canned: "hello there friend".into(),
        canned_segments: vec![
            SttSegment {
                start: 0.0,
                end: 1.0,
                text: "hello".into(),
            },
            SttSegment {
                start: 1.0,
                end: 2.5,
                text: "there friend".into(),
            },
        ],
        ..Default::default()
    });
    let s = setup_with_stt(Some(mock)).await;

    // Use the DM (alice is a member) so both the participant gate on /audio and
    // the access gate on the export pass for the same session.
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);

    let (st, _) = post_raw(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/audio"),
        b"fake-opus-bytes",
        "audio/webm",
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 1);
    assert_eq!(
        segs[0].duration_ms, 2500,
        "the real spoken duration from the engine's segments is stored"
    );

    // The VTT export uses the real duration: the cue is exactly 2.5s long, not
    // the synthetic 2-3s fallback. Its START is the wall-clock gap between the
    // /start and /audio POSTs, which the test does not control, so assert the
    // LENGTH exactly and only bound the start (LC-752: before sub-second
    // storage that gap rounded to 0 or 1 whole second at random).
    let (st, vtt) = get(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/export?format=vtt"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (start_ms, end_ms) = first_cue_ms(&vtt);
    assert_eq!(end_ms - start_ms, 2500, "cue uses the real duration: {vtt}");
    assert!(
        start_ms < 5_000,
        "cue starts at the real offset of two back-to-back POSTs: {vtt}"
    );
}

/// The first cue's `(start_ms, end_ms)` from a WebVTT body.
fn first_cue_ms(vtt: &str) -> (i64, i64) {
    let line = vtt
        .lines()
        .find(|l| l.contains(" --> "))
        .unwrap_or_else(|| panic!("no cue timing line in: {vtt}"));
    let (start, end) = line.split_once(" --> ").unwrap();
    (vtt_ms(start), vtt_ms(end))
}

/// "HH:MM:SS.mmm" -> milliseconds.
fn vtt_ms(t: &str) -> i64 {
    let parts: Vec<&str> = t.trim().split(':').collect();
    assert_eq!(parts.len(), 3, "bad VTT timestamp: {t}");
    let (secs, ms) = parts[2]
        .split_once('.')
        .unwrap_or_else(|| panic!("bad VTT timestamp: {t}"));
    parts[0].parse::<i64>().unwrap() * 3_600_000
        + parts[1].parse::<i64>().unwrap() * 60_000
        + secs.parse::<i64>().unwrap() * 1_000
        + ms.parse::<i64>().unwrap()
}

/// LC-752: the cue offset is `spoken_at - started_at` at millisecond
/// resolution. With known fractional timestamps the exported cue start is
/// exact, so a regression to whole-second storage or whole-second parsing
/// fails here instead of passing by luck.
#[tokio::test]
async fn export_vtt_cue_offset_keeps_sub_second_precision() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=hello+world"),
    )
    .await;

    // Pin both timestamps 1.25s apart, within the same whole second.
    sqlx::query("UPDATE call_transcripts SET started_at = '2026-01-01 00:00:00.500' WHERE id = ?")
        .bind(tid)
        .execute(&s.chat)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE transcript_segments SET spoken_at = '2026-01-01 00:00:01.750' \
         WHERE transcript_id = ?",
    )
    .bind(tid)
    .execute(&s.chat)
    .await
    .unwrap();

    let (st, vtt) = get(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/export?format=vtt"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (start_ms, _) = first_cue_ms(&vtt);
    assert_eq!(
        start_ms, 1_250,
        "cue offset keeps the sub-second part (whole-second rounding gives 1000): {vtt}"
    );

    // The page and the txt header still show whole seconds; the fraction is
    // precision for the cue maths, not for a human reading a timestamp.
    let (st, txt) = get(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/export?format=txt"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        txt.contains("2026-01-01 00:00:00\n") && !txt.contains("00:00:00.500"),
        "txt header shows the whole-second timestamp: {txt}"
    );
}

/// LC-752: rows written before the millisecond change still export at their
/// real offset instead of collapsing to the start of the call.
#[tokio::test]
async fn export_vtt_still_parses_legacy_whole_second_timestamps() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=hello+world"),
    )
    .await;

    sqlx::query("UPDATE call_transcripts SET started_at = '2026-01-01 00:00:00' WHERE id = ?")
        .bind(tid)
        .execute(&s.chat)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE transcript_segments SET spoken_at = '2026-01-01 00:00:07' WHERE transcript_id = ?",
    )
    .bind(tid)
    .execute(&s.chat)
    .await
    .unwrap();

    let (st, vtt) = get(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/export?format=vtt"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (start_ms, _) = first_cue_ms(&vtt);
    assert_eq!(
        start_ms, 7_000,
        "the whole-second form still parses (0 would mean it fell through): {vtt}"
    );
}

/// LC-752: the sessions and segments the app writes carry millisecond
/// precision, so the cue offset above is real rather than quantized.
#[tokio::test]
async fn stored_transcript_timestamps_carry_milliseconds() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=hello+world"),
    )
    .await;

    let session = db::transcripts::get(&s.chat, tid).await.unwrap().unwrap();
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 1);
    for (label, ts) in [
        ("started_at", &session.started_at),
        ("spoken_at", &segs[0].spoken_at),
    ] {
        assert_eq!(
            ts.len(),
            "2026-01-01 00:00:00.000".len(),
            "{label} is stored with milliseconds, got {ts}"
        );
        assert_eq!(
            ts.as_bytes().get(19),
            Some(&b'.'),
            "{label} is stored with milliseconds, got {ts}"
        );
    }
}

#[tokio::test]
async fn export_txt_and_vtt() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=hello+world"),
    )
    .await;
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/end"),
        None,
    )
    .await;

    // txt export contains the line; member only.
    let (st, txt) = get(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/export?format=txt"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(txt.contains("hello world"), "txt export body: {txt}");

    // vtt export is a valid WebVTT with a cue + the text.
    let (st, vtt) = get(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/export?format=vtt"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        vtt.starts_with("WEBVTT"),
        "vtt must start with WEBVTT: {vtt}"
    );
    assert!(vtt.contains("-->"), "vtt must have a cue timing: {vtt}");
    assert!(vtt.contains("hello world"));

    // Non-member cannot export.
    let (st, _) = get(
        &s.app,
        &s.outsider_session,
        &format!("/transcripts/{tid}/export?format=txt"),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn archive_search_filters_by_content() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=quarterly+roadmap+review"),
    )
    .await;
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/end"),
        None,
    )
    .await;

    // Matching query lists it.
    let (st, page) = get(&s.app, &s.a_session, "/transcripts?q=roadmap").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        page.contains(&format!("/transcripts/{tid}")),
        "matching search lists the transcript"
    );

    // Non-matching query does not.
    let (st, page) = get(&s.app, &s.a_session, "/transcripts?q=zzzznope").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        !page.contains(&format!("/transcripts/{tid}")),
        "non-matching search excludes it"
    );
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

// ---- LC-441/LC-442: delete authorization ----------------------------------

async fn del(app: &Router, sess: &str, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Pull the transcript id out of the `/start` JSON (`{"transcript_id":N}`)
/// without pulling in a json dep for one number.
fn extract_id(body: &str) -> i64 {
    body.split("transcript_id")
        .nth(1)
        .unwrap()
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn starter_can_delete_their_transcript() {
    let s = setup().await;
    let (st, body) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id = extract_id(&body);

    let (st, body) = del(&s.app, &s.b_session, &format!("/transcripts/{id}")).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("lc-toast"), "delete should confirm: {body}");
    assert!(
        db::transcripts::get(&s.chat, id).await.unwrap().is_none(),
        "transcript should be gone"
    );
}

#[tokio::test]
async fn member_who_is_not_starter_cannot_delete() {
    let s = setup().await;
    // alice (admin) starts it; bob is a DM member (can view) but not the starter.
    let (st, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id = extract_id(&body);

    let (st, _) = del(&s.app, &s.b_session, &format!("/transcripts/{id}")).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(
        db::transcripts::get(&s.chat, id).await.unwrap().is_some(),
        "a non-starter non-admin must not delete it"
    );
}

#[tokio::test]
async fn outsider_cannot_delete_transcript() {
    let s = setup().await;
    let (st, body) = post(
        &s.app,
        &s.b_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id = extract_id(&body);

    // carol is not a DM member -> require_access blocks her before the owner rule.
    let (st, _) = del(&s.app, &s.outsider_session, &format!("/transcripts/{id}")).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(db::transcripts::get(&s.chat, id).await.unwrap().is_some());
}

// ── LC-483: voice-message transcription ───────────────────────────────────────

/// A voice message (audio upload + waveform) gets transcribed via the STT
/// client and the text lands on the attachment that the message render reads.
#[tokio::test]
async fn voice_message_is_transcribed_and_attached() {
    let s = setup_with_stt(Some(Arc::new(lets_chat::stt::MockSttClient::text(
        "hello from whisper",
    ))))
    .await;

    let rel = "lc483-voice.webm";
    std::fs::write(db::uploads_dir().join(rel), b"fake-audio-bytes").unwrap();
    let upload_id = db::uploads::insert_upload(
        &s.chat,
        &s.b_id,
        "voice.webm",
        "audio/webm",
        16,
        rel,
        Some(r#"{"d":1.5,"p":[0.1,0.4,0.2]}"#),
    )
    .await
    .unwrap();
    let mid = db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "\u{1f3a4}")
        .await
        .unwrap();
    db::uploads::link_upload_to_message(&s.chat, upload_id, mid)
        .await
        .unwrap();

    routes::maybe_transcribe_voice_message(&s.state, upload_id, mid, s.public_room)
        .await
        .unwrap();

    let atts = db::uploads::attachments_for_message(&s.chat, mid)
        .await
        .unwrap();
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0].transcript.as_deref(), Some("hello from whisper"));
    assert!(
        atts[0].waveform.is_some(),
        "still rendered as a voice message"
    );
}

/// LC-496: a video clip (a `video/*` upload, no waveform) is transcribed the
/// same way a voice message is - the container is forwarded to STT and the
/// transcript lands on the attachment the message render reads.
#[tokio::test]
async fn video_clip_is_transcribed_and_attached() {
    let s = setup_with_stt(Some(Arc::new(lets_chat::stt::MockSttClient::text(
        "clip transcript text",
    ))))
    .await;

    let rel = "lc496-clip.webm";
    std::fs::write(db::uploads_dir().join(rel), b"fake-video-bytes").unwrap();
    let upload_id = db::uploads::insert_upload(
        &s.chat,
        &s.b_id,
        "clip.webm",
        "video/webm",
        16,
        rel,
        None, // clips carry no waveform
    )
    .await
    .unwrap();
    let mid = db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "clip")
        .await
        .unwrap();
    db::uploads::link_upload_to_message(&s.chat, upload_id, mid)
        .await
        .unwrap();

    routes::maybe_transcribe_voice_message(&s.state, upload_id, mid, s.public_room)
        .await
        .unwrap();

    let atts = db::uploads::attachments_for_message(&s.chat, mid)
        .await
        .unwrap();
    assert_eq!(atts.len(), 1);
    assert!(atts[0].is_video(), "rendered as a video clip");
    assert_eq!(atts[0].transcript.as_deref(), Some("clip transcript text"));
}

/// A non-voice upload (image: no waveform) is never sent to STT, even with a
/// working client configured.
#[tokio::test]
async fn non_voice_upload_is_not_transcribed() {
    let s = setup_with_stt(Some(Arc::new(lets_chat::stt::MockSttClient::text(
        "should not be used",
    ))))
    .await;

    let rel = "lc483-image.png";
    std::fs::write(db::uploads_dir().join(rel), b"not-really-a-png").unwrap();
    let upload_id =
        db::uploads::insert_upload(&s.chat, &s.b_id, "pic.png", "image/png", 16, rel, None)
            .await
            .unwrap();
    let mid = db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "pic")
        .await
        .unwrap();
    db::uploads::link_upload_to_message(&s.chat, upload_id, mid)
        .await
        .unwrap();

    routes::maybe_transcribe_voice_message(&s.state, upload_id, mid, s.public_room)
        .await
        .unwrap();

    let (row, _) = db::uploads::get_upload(&s.chat, upload_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.transcript, None);
}

/// With STT disabled the helper is a no-op, even for a real voice message.
#[tokio::test]
async fn voice_message_not_transcribed_when_stt_disabled() {
    let s = setup().await; // stt_client = None

    let rel = "lc483-voice-off.webm";
    std::fs::write(db::uploads_dir().join(rel), b"fake-audio-bytes").unwrap();
    let upload_id = db::uploads::insert_upload(
        &s.chat,
        &s.b_id,
        "voice.webm",
        "audio/webm",
        16,
        rel,
        Some(r#"{"d":1.0,"p":[0.2]}"#),
    )
    .await
    .unwrap();

    routes::maybe_transcribe_voice_message(&s.state, upload_id, 0, s.public_room)
        .await
        .unwrap();

    let (row, _) = db::uploads::get_upload(&s.chat, upload_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.transcript, None);
}

// ── LC-590: STT reliability (retry, failed status, retranscribe) ──────────────

/// Insert a linked voice-message upload owned by bob and return
/// `(upload_id, message_id)`. Every LC-590 test needs the same fixture.
async fn seed_voice_message(s: &Setup, rel: &str) -> (i64, i64) {
    std::fs::write(db::uploads_dir().join(rel), b"fake-audio-bytes").unwrap();
    let upload_id = db::uploads::insert_upload(
        &s.chat,
        &s.b_id,
        "voice.webm",
        "audio/webm",
        16,
        rel,
        Some(r#"{"d":1.5,"p":[0.1,0.4,0.2]}"#),
    )
    .await
    .unwrap();
    let mid = db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "\u{1f3a4}")
        .await
        .unwrap();
    db::uploads::link_upload_to_message(&s.chat, upload_id, mid)
        .await
        .unwrap();
    (upload_id, mid)
}

/// A transient engine failure is retried, and the clip is transcribed on a later
/// attempt instead of being lost. This exercises the REAL production backoff
/// (the unit tests inject zero delays), so it proves the retry is actually wired
/// into the voice path and not just testable in isolation.
#[tokio::test]
async fn transient_stt_failure_is_retried_then_succeeds() {
    let mock = Arc::new(lets_chat::stt::MockSttClient::failing("recovered text", 1));
    let s = setup_with_stt(Some(mock.clone())).await;
    let (upload_id, mid) = seed_voice_message(&s, "lc590-retry.webm").await;

    routes::maybe_transcribe_voice_message(&s.state, upload_id, mid, s.public_room)
        .await
        .unwrap();

    assert_eq!(
        mock.call_count(),
        2,
        "first attempt failed, second succeeded"
    );
    let (row, _) = db::uploads::get_upload(&s.chat, upload_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.transcript.as_deref(), Some("recovered text"));
    assert_eq!(
        row.transcript_status.as_deref(),
        Some("done"),
        "a recovered clip is not left looking failed"
    );
}

/// Once the attempts are exhausted the failure is PERSISTED rather than only
/// logged, and the attachment the renderer reads reports it - which is what puts
/// the Retry control on screen. Uses a permanent 4xx so the test does not sit
/// through the production backoff; the retry counting is unit-tested separately.
#[tokio::test]
async fn exhausted_stt_failure_marks_the_upload_failed() {
    let mock = Arc::new(lets_chat::stt::MockSttClient::failing_permanently());
    let s = setup_with_stt(Some(mock.clone())).await;
    let (upload_id, mid) = seed_voice_message(&s, "lc590-failed.webm").await;

    routes::maybe_transcribe_voice_message(&s.state, upload_id, mid, s.public_room)
        .await
        .unwrap();

    assert_eq!(mock.call_count(), 1, "a 4xx is not retried");
    let atts = db::uploads::attachments_for_message(&s.chat, mid)
        .await
        .unwrap();
    assert_eq!(atts[0].transcript, None);
    assert!(
        atts[0].transcript_failed(),
        "the renderer must be able to see the failure"
    );
    assert!(!atts[0].transcript_pending());
}

/// An engine that runs fine but hears nothing is NOT a failure: a silent clip
/// must not sprout a Retry control that would just fail the same way again.
#[tokio::test]
async fn silent_clip_is_not_marked_failed() {
    let s = setup_with_stt(Some(Arc::new(lets_chat::stt::MockSttClient::text("   ")))).await;
    let (upload_id, mid) = seed_voice_message(&s, "lc590-silent.webm").await;

    routes::maybe_transcribe_voice_message(&s.state, upload_id, mid, s.public_room)
        .await
        .unwrap();

    let atts = db::uploads::attachments_for_message(&s.chat, mid)
        .await
        .unwrap();
    assert_eq!(atts[0].transcript, None);
    assert!(!atts[0].transcript_failed(), "silence is not a failure");
}

/// The uploader can retry a failed transcription, and only the uploader can:
/// the route's gate matches the control's `message.can_edit` visibility exactly.
#[tokio::test]
async fn retranscribe_route_is_uploader_only_and_requeues() {
    let s = setup_with_stt(Some(Arc::new(lets_chat::stt::MockSttClient::text(
        "second time lucky",
    ))))
    .await;
    let (upload_id, mid) = seed_voice_message(&s, "lc590-retranscribe.webm").await;
    db::uploads::set_transcript_status(&s.chat, upload_id, db::uploads::TRANSCRIPT_FAILED)
        .await
        .unwrap();

    let uri = format!("/api/files/{upload_id}/retranscribe");
    // alice did not upload this clip.
    let (st, _) = post(&s.app, &s.a_session, &uri, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (row, _) = db::uploads::get_upload(&s.chat, upload_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.transcript_status.as_deref(),
        Some("failed"),
        "a rejected request must not touch the row"
    );

    // A non-transcribable attachment is rejected rather than being parked in
    // `pending` forever by a helper that would no-op on it.
    let img_rel = "lc590-not-audio.png";
    std::fs::write(db::uploads_dir().join(img_rel), b"not-really-a-png").unwrap();
    let img_id =
        db::uploads::insert_upload(&s.chat, &s.b_id, "pic.png", "image/png", 16, img_rel, None)
            .await
            .unwrap();
    let img_mid = db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "pic")
        .await
        .unwrap();
    db::uploads::link_upload_to_message(&s.chat, img_id, img_mid)
        .await
        .unwrap();
    let (st, _) = post(
        &s.app,
        &s.b_session,
        &format!("/api/files/{img_id}/retranscribe"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (img_row, _) = db::uploads::get_upload(&s.chat, img_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(img_row.transcript_status, None, "never marked pending");

    // bob did.
    let (st, body) = post(&s.app, &s.b_session, &uri, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body.contains(&format!("msg-{mid}")),
        "responds with the re-rendered message"
    );

    // The work is spawned, so poll briefly for it rather than racing it.
    let mut transcript = None;
    for _ in 0..50 {
        let (row, _) = db::uploads::get_upload(&s.chat, upload_id)
            .await
            .unwrap()
            .unwrap();
        if row.transcript.is_some() {
            transcript = row.transcript;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(transcript.as_deref(), Some("second time lucky"));
}

/// A call clip whose transcription fails answers non-2xx. Pre-LC-590 this was a
/// 200 with an empty body, indistinguishable from "the speaker said nothing",
/// so the client had nothing to show a caption-failed state from.
#[tokio::test]
async fn failed_call_clip_returns_non_2xx() {
    let s = setup_with_stt(Some(Arc::new(
        lets_chat::stt::MockSttClient::failing_permanently(),
    )))
    .await;
    let (st, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let tid = extract_id(&body);

    let (st, _) = post_raw(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/audio"),
        b"fake-audio",
        "audio/webm",
    )
    .await;
    assert!(
        !st.is_success(),
        "a dropped clip must be visible to the client, got {st}"
    );
}

// ── LC-484: AI catch-me-up summaries (threads + channel) ──────────────────────

fn with_llm(canned: &str) -> Option<std::sync::Arc<dyn lets_chat::llm::LlmClient>> {
    Some(std::sync::Arc::new(lets_chat::llm::MockLlmClient {
        canned: canned.to_string(),
    }))
}

/// POST /room/{id}/summary summarizes the room's messages via the LLM client
/// and renders the result markdown into the fragment.
#[tokio::test]
async fn channel_summary_generates_from_messages() {
    let s = setup_with_clients(None, with_llm("CANNED_CHANNEL_SUMMARY")).await;
    db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "hello team")
        .await
        .unwrap();
    db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "ship it friday")
        .await
        .unwrap();

    let (st, body) = post(
        &s.app,
        &s.a_session,
        &format!("/room/{}/summary", s.public_room),
        Some(""),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("CANNED_CHANNEL_SUMMARY"), "rendered: {body}");
}

/// With no LLM configured the action is rejected (the button is also hidden).
#[tokio::test]
async fn channel_summary_400_when_llm_disabled() {
    let s = setup().await;
    db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "hi")
        .await
        .unwrap();
    let (st, _) = post(
        &s.app,
        &s.a_session,
        &format!("/room/{}/summary", s.public_room),
        Some(""),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// POST /room/{id}/thread/{parent}/summary summarizes the thread root (+ any
/// replies) via the LLM client.
#[tokio::test]
async fn thread_summary_generates() {
    let s = setup_with_clients(None, with_llm("CANNED_THREAD_SUMMARY")).await;
    let parent = db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "root question")
        .await
        .unwrap();

    let (st, body) = post(
        &s.app,
        &s.a_session,
        &format!("/room/{}/thread/{}/summary", s.public_room, parent),
        Some(""),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("CANNED_THREAD_SUMMARY"), "rendered: {body}");
}

/// GET /room/{id}/summary opens the catch-me-up drawer with a Generate button.
#[tokio::test]
async fn catch_me_up_panel_opens() {
    let s = setup_with_clients(None, with_llm("x")).await;
    let (st, body) = get(
        &s.app,
        &s.a_session,
        &format!("/room/{}/summary", s.public_room),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("room-summary-body"), "drawer body: {body}");
}

// ── LC-486: inline message translation ────────────────────────────────────────

// LC-694: the first Translate click (empty body, no `lang`) opens the language
// picker and runs NO LLM call, so nothing is translated or cached until the user
// confirms a target. Confirming a target (`lang=`) translates once and caches by
// that target's code.
#[tokio::test]
async fn translate_opens_picker_then_translates_and_caches() {
    let s = setup_with_clients(None, with_llm("HOLA_TRANSLATED")).await;
    let mid = db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "hello there")
        .await
        .unwrap();

    // First click: picker only, no translation, nothing cached.
    let (st, body) = post(
        &s.app,
        &s.a_session,
        &format!("/messages/{mid}/translate"),
        Some(""),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        !body.contains("HOLA_TRANSLATED"),
        "opening the picker must not translate: {body}"
    );
    assert!(
        body.contains("lc-translation-select"),
        "picker rendered: {body}"
    );
    let cached = db::translations::get_cached(&s.chat, mid, &lets_chat::i18n::current_lang_code())
        .await
        .unwrap();
    assert_eq!(
        cached, None,
        "no translation cached from opening the picker"
    );

    // Confirming a target translates once and caches under that target's code.
    let (st, body) = post(
        &s.app,
        &s.a_session,
        &format!("/messages/{mid}/translate"),
        Some("lang=es"),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        body.contains("HOLA_TRANSLATED"),
        "rendered translation: {body}"
    );
    let cached = db::translations::get_cached(&s.chat, mid, "es")
        .await
        .unwrap();
    assert_eq!(cached.as_deref(), Some("HOLA_TRANSLATED"));
}

#[tokio::test]
async fn translate_400_when_llm_disabled() {
    let s = setup().await; // no llm client
    let mid = db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "hello")
        .await
        .unwrap();
    let (st, _) = post(
        &s.app,
        &s.a_session,
        &format!("/messages/{mid}/translate"),
        Some(""),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

// The cache round-trips and `delete_for_message` (called from the edit path)
// clears it, so a re-translation reflects the edited body.
// ── LC-629: AI-corrected live transcript, raw recognition retained ────────────

/// An `LlmClient` double that fixes a fixed set of speech-to-text recognition
/// errors on the NEW line (input-dependent, unlike the crate's canned
/// `MockLlmClient`) and records the largest context window it was handed, so the
/// rolling-window bound is observable from a test.
#[derive(Default)]
struct CorrectingMock {
    max_context_seen: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl lets_chat::llm::LlmClient for CorrectingMock {
    async fn complete(
        &self,
        _system: &str,
        user_content: &str,
    ) -> Result<String, lets_chat::llm::LlmError> {
        // The default `correct` frames the context as `- ` bullets then the new
        // line after this marker. Count the bullets (the window it saw) and
        // correct the new line against a small canned map; pass others through.
        let ctx_lines = user_content.lines().filter(|l| l.starts_with("- ")).count();
        self.max_context_seen
            .fetch_max(ctx_lines, std::sync::atomic::Ordering::Relaxed);
        let line = user_content
            .rsplit("NEW line to correct:\n")
            .next()
            .unwrap_or("")
            .trim();
        let fixed = match line {
            "i program in java script" => "I program in JavaScript",
            "we use post gres" => "we use Postgres",
            other => other,
        };
        Ok(fixed.to_string())
    }
}

/// Live captions are the raw speech recognition, verbatim, even when an LLM is
/// configured. An earlier build ran each recognized line through a per-segment
/// LLM pass before display (LC-629); on a self-hosted model that surfaced the
/// model's reply to the line instead of the words and added its latency to every
/// caption, so the live path no longer consults the LLM at all. This guards
/// against reintroducing it: a "correcting" LLM is configured, yet the stored and
/// displayed caption is the exact recognized text. The LLM is for the post-call
/// summary / brief only.
#[tokio::test]
async fn live_captions_are_verbatim_even_with_llm_configured() {
    let mock: Arc<dyn lets_chat::llm::LlmClient> = Arc::new(CorrectingMock::default());
    let s = setup_with_clients(None, Some(mock)).await;

    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);

    // A phrase the mock WOULD rewrite if the live path fed it to the LLM.
    let (st, _) = post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=i+program+in+java+script"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs.len(), 1);
    // Stored verbatim: the LLM never touched the live caption.
    assert_eq!(segs[0].text, "i program in java script");
    assert_eq!(segs[0].raw_text, None, "no separate raw is stored");

    // The saved page shows the verbatim recognition, not an LLM rewrite.
    let (st, page) = get(&s.app, &s.a_session, &format!("/transcripts/{tid}")).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        page.contains("i program in java script"),
        "page shows the verbatim caption: {page}"
    );
    assert!(
        !page.contains("JavaScript"),
        "the LLM must not rewrite live captions"
    );
}

/// With no LLM configured the display text is the raw recognition, stored
/// verbatim with no separate raw column.
#[tokio::test]
async fn without_llm_display_equals_raw() {
    let s = setup().await; // llm_client = None
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=verbatim+text"),
    )
    .await;
    let segs = db::transcripts::list_segments(&s.chat, tid).await.unwrap();
    assert_eq!(segs[0].text, "verbatim text");
    assert_eq!(segs[0].raw_text, None);
}

// ── LC-792: no automatic post-call recap; summaries are on-demand only ────────

/// LC-792: ending a call never auto-posts an AI recap into the room - not even
/// with the assistant opted in AND an LLM configured. Summaries are on-demand
/// only (the transcript page's Summarize action). Guards against the recap
/// creeping back onto the finalize path.
#[tokio::test]
async fn call_end_does_not_auto_post_recap() {
    let s = setup_with_clients(None, with_llm("## Summary\nShould never be auto-posted.")).await;
    // Opt the assistant in - the strongest case for the old auto-recap - so this
    // proves removal, not just a gate that happened to be off.
    db::chat::set_room_assistant_enabled(&s.chat, s.dm_room, true)
        .await
        .unwrap();

    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=hello+there"),
    )
    .await;
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/end"),
        None,
    )
    .await;

    // Give any spawned work a chance to (not) run.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let msgs = db::chat::list_messages(&s.chat, s.dm_room).await.unwrap();
    assert!(
        !msgs.iter().any(|m| m.body.contains("Call recap")),
        "call end must not auto-post a recap"
    );
    let t = db::transcripts::get(&s.chat, tid).await.unwrap().unwrap();
    assert!(t.summary.is_none(), "no summary is generated automatically");
}

/// LC-663: an `LlmClient` double that records the user content (the transcript
/// text) it was handed, so the speaker roster + attributed lines the summary
/// sends are observable without a live model.
#[derive(Default)]
struct CapturingLlm {
    last_user: std::sync::Mutex<Option<String>>,
    canned: String,
}

#[async_trait::async_trait]
impl lets_chat::llm::LlmClient for CapturingLlm {
    async fn complete(
        &self,
        _system: &str,
        user_content: &str,
    ) -> Result<String, lets_chat::llm::LlmError> {
        *self.last_user.lock().unwrap() = Some(user_content.to_string());
        Ok(self.canned.clone())
    }
}

/// LC-663: the on-demand summary sends the model a `Participants:` roster and
/// speaker-prefixed lines, so it can attribute decisions and action items to the
/// people who spoke.
#[tokio::test]
async fn summary_sends_speaker_roster_and_attributed_lines() {
    let cap = Arc::new(CapturingLlm {
        canned: "## Summary\nThey decided.\n\n## Decisions\n- alice - ship Friday".to_string(),
        ..Default::default()
    });
    let llm: Arc<dyn lets_chat::llm::LlmClient> = cap.clone();
    let s = setup_with_clients(None, Some(llm)).await;

    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    // Two distinct speakers: alice and bob each transcribe their own mic.
    post(
        &s.app,
        &s.a_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=we+will+ship+friday"),
    )
    .await;
    post(
        &s.app,
        &s.b_session,
        &format!("/call/transcript/{tid}/segment"),
        Some("text=i+will+own+the+docs"),
    )
    .await;

    // The on-demand summary (the transcript page's Summarize action) is what now
    // calls the model - there is no automatic recap.
    let (st, _) = post(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/summary"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let seen = cap
        .last_user
        .lock()
        .unwrap()
        .clone()
        .expect("the model received the transcript");
    assert!(
        seen.contains("Participants: alice, bob"),
        "roster leads the prompt: {seen}"
    );
    assert!(
        seen.contains("alice: we will ship friday"),
        "alice's line is attributed: {seen}"
    );
    assert!(
        seen.contains("bob: i will own the docs"),
        "bob's line is attributed: {seen}"
    );
}

// ── LC-664: personal "what did I miss" brief ──────────────────────────────────

/// A member gets a brief personalized to them: the model is handed the viewer's
/// name plus the transcript, and the rendered brief comes back. A non-member is
/// refused.
#[tokio::test]
async fn brief_is_personalized_to_the_viewer() {
    let cap = Arc::new(CapturingLlm {
        canned: "You have one action item: write the docs.".to_string(),
        ..Default::default()
    });
    let llm: Arc<dyn lets_chat::llm::LlmClient> = cap.clone();
    let s = setup_with_clients(None, Some(llm)).await;

    // Start a session in the DM, then seed a segment spoken by bob (alice, the
    // viewer, missed it). Seed via the db to skip the live-correction path.
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    db::transcripts::append_segment(&s.chat, tid, &s.b_id, "we shipped the release", None, 0)
        .await
        .unwrap();

    // alice asks for her brief.
    let (st, frag) = post(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/brief"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{frag}");
    assert!(frag.contains("write the docs"), "brief rendered: {frag}");

    // The model was told who to brief, with the transcript as data.
    let seen = cap
        .last_user
        .lock()
        .unwrap()
        .clone()
        .expect("the model received the brief request");
    assert!(
        seen.contains("Person to brief: alice"),
        "brief names the viewer: {seen}"
    );
    assert!(
        seen.contains("bob: we shipped the release"),
        "brief carries the transcript: {seen}"
    );

    // A non-member cannot brief themselves on someone else's call.
    let (st, _) = post(
        &s.app,
        &s.outsider_session,
        &format!("/transcripts/{tid}/brief"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

/// With no LLM configured the brief is refused (the card is also hidden).
#[tokio::test]
async fn brief_400_when_llm_disabled() {
    let s = setup().await;
    let (_, body) = post(
        &s.app,
        &s.a_session,
        &format!("/call/{}/transcript/start", s.dm_room),
        None,
    )
    .await;
    let tid = parse_id(&body);
    db::transcripts::append_segment(&s.chat, tid, &s.b_id, "hello", None, 0)
        .await
        .unwrap();
    let (st, _) = post(
        &s.app,
        &s.a_session,
        &format!("/transcripts/{tid}/brief"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn translation_cache_upsert_and_invalidate() {
    let s = setup().await;
    let mid = db::chat::insert_message(&s.chat, s.public_room, &s.b_id, "hi")
        .await
        .unwrap();
    db::translations::upsert(&s.chat, mid, "es", "hola")
        .await
        .unwrap();
    assert_eq!(
        db::translations::get_cached(&s.chat, mid, "es")
            .await
            .unwrap()
            .as_deref(),
        Some("hola")
    );
    db::translations::delete_for_message(&s.chat, mid)
        .await
        .unwrap();
    assert!(db::translations::get_cached(&s.chat, mid, "es")
        .await
        .unwrap()
        .is_none());
}
