//! LC-494: stage control-plane unit tests - the ephemeral hub roster and the
//! per-room toggle. (The WS frames + per-viewer render are integration-level;
//! these cover the state machine + persistence.)

use lets_chat::db::chat;
use lets_chat::ws::hub::Hub;

mod common;

/// LC-596/LC-610: both LiveKit tests mutate the process-global
/// `LETS_CHAT_LIVEKIT_*` env, which `livekit::available()` reads. Tests in one
/// binary run on parallel threads, so they must not overlap. Each locks this for
/// its whole body. A `tokio::sync::Mutex` (not `std`) because the guard is held
/// across awaits; it has no poisoning, so a panicking test just releases it.
static LIVEKIT_ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[test]
fn stage_roster_promote_demote_hands_and_leave() {
    let hub = Hub::new();
    let room = 7;

    // Everyone joins as a listener.
    for u in ["host", "alice", "bob"] {
        hub.stage_join(room, u);
    }
    let r = hub.stage_roster(room).unwrap();
    assert_eq!(r.participants.len(), 3);
    assert!(r.speakers.is_empty());

    // alice raises a hand; bob does not.
    assert!(hub.stage_raise_hand(room, "alice"));
    assert!(hub.stage_roster(room).unwrap().hands.contains("alice"));

    // host promotes alice -> speaker, hand cleared.
    hub.stage_promote(room, "alice");
    let r = hub.stage_roster(room).unwrap();
    assert!(r.speakers.contains("alice"));
    assert!(!r.hands.contains("alice"));

    // A speaker cannot also have a raised hand.
    assert!(!hub.stage_raise_hand(room, "alice"));

    // Demote alice back to listener.
    hub.stage_demote(room, "alice");
    assert!(!hub.stage_roster(room).unwrap().speakers.contains("alice"));

    // Leaving removes from the roster; emptying the room drops the entry.
    for u in ["host", "alice", "bob"] {
        hub.stage_leave(room, u);
    }
    assert!(hub.stage_roster(room).is_none());
}

#[test]
fn stage_leave_all_reports_affected_rooms() {
    let hub = Hub::new();
    hub.stage_join(1, "u");
    hub.stage_join(2, "u");
    hub.stage_join(2, "other");
    let mut affected = hub.stage_leave_all("u");
    affected.sort();
    assert_eq!(affected, vec![1, 2]);
    // Room 1 emptied -> gone; room 2 still has `other`.
    assert!(hub.stage_roster(1).is_none());
    assert!(hub.stage_roster(2).unwrap().participants.contains("other"));
}

#[tokio::test]
async fn room_stage_toggle_roundtrips() {
    let pool = common::chat_pool().await;
    let room = chat::create_room(&pool, "stage", None, "public", None, None)
        .await
        .unwrap();
    assert!(!chat::get_room_stage_enabled(&pool, room).await.unwrap());
    assert_eq!(
        chat::set_room_stage_enabled(&pool, room, true)
            .await
            .unwrap(),
        1
    );
    assert!(chat::get_room_stage_enabled(&pool, room).await.unwrap());
}

/// LC-597: the stage panel is what gates the transcribe control in the UI, and
/// it re-renders on every roster change. Speakers get the toggle; listeners must
/// not, because the server would refuse their session anyway
/// (`require_participant` -> `is_stage_speaker`) and offering a dead button is
/// worse than offering none.
///
/// This is also the first render-level test for the panel at all.
#[tokio::test]
async fn stage_panel_offers_transcription_to_speakers_only() {
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
    use std::sync::Arc;
    use tower::ServiceExt;

    let dir = std::env::temp_dir().join(format!("lc-stage-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let uid = db::auth::create_user(&auth, "speaker", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&uid)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &uid).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    // General (room 1) with the stage switched on.
    let room = 1;
    chat::set_room_stage_enabled(&chat, room, true)
        .await
        .unwrap();

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
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    let app = routes::build_router(state);

    async fn room_page(app: &axum::Router, session: &str, room: i64) -> String {
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("/room/{room}"))
            .header("cookie", format!("session={session}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), 1 << 22).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    // On the stage as a listener: no transcribe control.
    hub.stage_join(room, &uid);
    let s = room_page(&app, &session, room).await;
    assert!(
        s.contains("data-lc-stage"),
        "precondition: the stage panel renders"
    );
    assert!(
        !s.contains("data-lc-stage-transcribe"),
        "a listener must not be offered the transcribe toggle"
    );

    // Granted the floor: the toggle appears, carrying the room id transcribe.js
    // resolves the session from.
    hub.stage_promote(room, &uid);
    let s = room_page(&app, &session, room).await;
    assert!(
        s.contains("data-lc-stage-transcribe"),
        "a speaker is offered the transcribe toggle"
    );
    assert!(
        s.contains(&format!(r#"data-lc-room="{room}""#)),
        "the toggle carries data-lc-room so transcribe.js can start a session"
    );
    assert!(
        s.contains("data-lc-transcript-panel-toggle"),
        "a speaker can open the transcript drawer"
    );
}

/// LC-596: the SFU token gate for a huddle.
///
/// Covers the four states that matter and are cheap to reach without a LiveKit
/// server: unconfigured (the mesh-fallback signal), configured but not in the
/// call, and finally a real token for a member.
///
/// LC-610: there is deliberately no participant-count threshold. A configured
/// huddle is entirely SFU regardless of size, so a lone member gets a token.
#[tokio::test]
async fn huddle_sfu_token_gate() {
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
    use std::sync::Arc;
    use tower::ServiceExt;

    let _env = LIVEKIT_ENV.lock().await;
    let dir = std::env::temp_dir().join(format!("lc-huddle-tok-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let uid = db::auth::create_user(&auth, "huddler", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&uid)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &uid).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let room = 1; // General

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
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    let app = routes::build_router(state);

    async fn token(app: &axum::Router, session: &str, room: i64) -> (StatusCode, String) {
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("/room/{room}/huddle/token"))
            .header("cookie", format!("session={session}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let st = res.status();
        let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (st, String::from_utf8_lossy(&body).into_owned())
    }

    // 1. LiveKit unconfigured. This is the mesh-fallback signal: the client
    //    keeps the WebRTC mesh and never reaches the SFU path.
    let (st, _) = token(&app, &session, room).await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "no LiveKit config must refuse, so the client stays on the mesh"
    );

    // Configure LiveKit for the remainder. `from_env` is read per call, so this
    // takes effect immediately.
    // SAFETY: single-threaded test; no other thread reads the env here.
    unsafe {
        std::env::set_var("LETS_CHAT_LIVEKIT_URL", "wss://lk.example.com");
        std::env::set_var("LETS_CHAT_LIVEKIT_API_KEY", "devkey");
        std::env::set_var(
            "LETS_CHAT_LIVEKIT_API_SECRET",
            "devsecretdevsecretdevsecret123456",
        );
    }

    // 2. Configured, but the caller is not in the huddle. Room access alone is
    //    not membership in the call.
    let (st, _) = token(&app, &session, room).await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "a room member who never joined the huddle gets no token"
    );

    // 3. A member of a configured huddle gets a token even when alone: the
    //    transport is fixed by config, not by count (LC-610). A huddle LiveKit
    //    room, with publish rights (symmetric - no listener role).
    let (conn, _rx, _) = hub.connect(&uid, "huddler");
    hub.voice_join(conn, room);
    let (st, body) = token(&app, &session, room).await;
    assert_eq!(
        st,
        StatusCode::OK,
        "a member of a configured huddle gets a token regardless of size: {body}"
    );
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["url"], "wss://lk.example.com");
    assert_eq!(v["can_publish"], true, "every huddle participant publishes");
    assert!(
        v["token"].as_str().is_some_and(|t| !t.is_empty()),
        "a token is issued"
    );

    unsafe {
        std::env::remove_var("LETS_CHAT_LIVEKIT_URL");
        std::env::remove_var("LETS_CHAT_LIVEKIT_API_KEY");
        std::env::remove_var("LETS_CHAT_LIVEKIT_API_SECRET");
    }
}

/// LC-610: the huddle root advertises its transport so voice.js can pick mesh
/// vs SFU before joining. The flag is a pure `livekit::available()` read, so it
/// flips with the env and nothing else.
#[tokio::test]
async fn huddle_root_advertises_the_sfu_transport() {
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
    use std::sync::Arc;
    use tower::ServiceExt;

    let _env = LIVEKIT_ENV.lock().await;
    // Ensure a clean baseline: another test may have left LiveKit configured.
    // SAFETY: guarded by LIVEKIT_ENV; no other test touches the env concurrently.
    unsafe {
        std::env::remove_var("LETS_CHAT_LIVEKIT_URL");
        std::env::remove_var("LETS_CHAT_LIVEKIT_API_KEY");
        std::env::remove_var("LETS_CHAT_LIVEKIT_API_SECRET");
    }

    let dir = std::env::temp_dir().join(format!("lc-huddle-root-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let uid = db::auth::create_user(&auth, "member", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&uid)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &uid).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let room = 1;

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
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
    let app = routes::build_router(state);

    async fn room_html(app: &axum::Router, session: &str, room: i64) -> String {
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("/room/{room}"))
            .header("cookie", format!("session={session}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), 1 << 22).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    // No LiveKit: the root reports the mesh.
    let s = room_html(&app, &session, room).await;
    assert!(
        s.contains(r#"data-lc-huddle-sfu="0""#),
        "with no LiveKit the huddle must use the mesh"
    );

    // Configure LiveKit: the same render now reports the SFU.
    unsafe {
        std::env::set_var("LETS_CHAT_LIVEKIT_URL", "wss://lk.example.com");
        std::env::set_var("LETS_CHAT_LIVEKIT_API_KEY", "devkey");
        std::env::set_var(
            "LETS_CHAT_LIVEKIT_API_SECRET",
            "devsecretdevsecretdevsecret123456",
        );
    }
    let s = room_html(&app, &session, room).await;
    assert!(
        s.contains(r#"data-lc-huddle-sfu="1""#),
        "with LiveKit configured the huddle must use the SFU"
    );

    unsafe {
        std::env::remove_var("LETS_CHAT_LIVEKIT_URL");
        std::env::remove_var("LETS_CHAT_LIVEKIT_API_KEY");
        std::env::remove_var("LETS_CHAT_LIVEKIT_API_SECRET");
    }
}

/// LC-612: the in-call indicator is correct on initial page load, not only on
/// later events - the sidebar seeds the count from the live hub. Renders the
/// badge for a room with an active huddle and an empty target otherwise.
#[tokio::test]
async fn sidebar_seeds_the_in_call_indicator_on_load() {
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
    use std::sync::Arc;
    use tower::ServiceExt;

    let dir = std::env::temp_dir().join(format!("lc-incall-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let uid = db::auth::create_user(&auth, "viewer", "h").await.unwrap();
    let other = db::auth::create_user(&auth, "caller", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&uid)
        .execute(&auth)
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &uid).await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let room = 1; // General, in the sidebar

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
        stt_client: None,
        llm_client: None,
        embedding_client: None,
    };
    let app = routes::build_router(state);

    async fn page(app: &axum::Router, session: &str, room: i64) -> String {
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("/room/{room}"))
            .header("cookie", format!("session={session}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), 1 << 22).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    // No huddle: the target span exists but is empty (no pill).
    let s = page(&app, &session, room).await;
    assert!(
        s.contains(&format!(r#"id="incall-room-{room}""#)),
        "the OOB target span is always present"
    );
    assert!(
        !s.contains("lc-incall-pill"),
        "no pill renders when nobody is in the huddle"
    );

    // Someone else joins the huddle for `room`; a fresh load now shows the pill.
    let (conn, _rx, _) = hub.connect(&other, "caller");
    hub.voice_join(conn, room);
    let s = page(&app, &session, room).await;
    assert!(
        s.contains("lc-incall-pill"),
        "an active huddle renders the in-call pill on load"
    );
}
