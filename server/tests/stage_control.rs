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
