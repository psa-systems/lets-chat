//! LC-494: stage control-plane unit tests - the ephemeral hub roster and the
//! per-room toggle. (The WS frames + per-viewer render are integration-level;
//! these cover the state machine + persistence.)

use lets_chat::db::chat;
use lets_chat::ws::hub::Hub;

mod common;

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
/// server: unconfigured (the mesh-fallback signal), not in the call, below the
/// participant threshold, and finally a real token above it.
#[tokio::test]
async fn huddle_sfu_token_gate() {
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use lets_chat::livekit::SFU_MIN_PARTICIPANTS;
    use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
    use std::sync::Arc;
    use tower::ServiceExt;

    let dir = std::env::temp_dir().join(format!("lc-huddle-tok-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test data dir");
    db::set_data_dir(dir.to_string_lossy().to_string());

    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let uid = db::auth::create_user(&auth, "huddler", "h").await.unwrap();
    // Extra users to push the roster over the threshold. They need no session:
    // `voice_room_users` dedupes by user id, so distinct users are required -
    // extra connections for the same user would not raise the count.
    let mut peers = Vec::new();
    for i in 1..SFU_MIN_PARTICIPANTS {
        peers.push(
            db::auth::create_user(&auth, &format!("peer{i}"), "h")
                .await
                .unwrap(),
        );
    }
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

    // 3. In the huddle but below the threshold: the mesh is still in charge, so
    //    a token here would split the call across two transports.
    let (conn, _rx, _) = hub.connect(&uid, "huddler");
    hub.voice_join(conn, room);
    let (st, body) = token(&app, &session, room).await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "below the threshold the mesh keeps the call: {body}"
    );

    // 4. At the threshold: a real token, for the huddle LiveKit room, with
    //    publish rights (a huddle is symmetric - no listener role).
    for (i, p) in peers.iter().enumerate() {
        let (c, _rx, _) = hub.connect(p, &format!("peer{}", i + 1));
        hub.voice_join(c, room);
    }
    assert_eq!(
        hub.voice_room_users(room).len(),
        SFU_MIN_PARTICIPANTS,
        "precondition: the roster is exactly at the threshold"
    );
    let (st, body) = token(&app, &session, room).await;
    assert_eq!(
        st,
        StatusCode::OK,
        "at the threshold the SFU takes over: {body}"
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
