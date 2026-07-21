//! LC-592: cost/load control for server-side STT - operator gating, rate
//! limiting, and bounded concurrency.
//!
//! This lives in its OWN test binary on purpose. The gating tests mutate
//! `LETS_CHAT_STT_SCOPE`, a process-global, and `stt_load::scope()` reads it on
//! every transcription. Sharing a process with `transcripts.rs` would let a
//! gating test switch transcription off underneath a concurrently-running test
//! that expects it on. Cargo gives each integration test file its own process,
//! so the blast radius of the env mutation is exactly this file - and within it,
//! `ENV_LOCK` serializes the tests that write.

use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};

mod common;

/// Serializes EVERY test in this file, not merely the ones that write.
/// `stt_load::scope()` reads `LETS_CHAT_STT_SCOPE` on each transcription, so a
/// test that only reads it is just as exposed to a concurrent writer: the rate
/// and concurrency tests failed outright until they took this too. A
/// `tokio::Mutex` because the burst test holds it across an await.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-stt-load-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

struct Setup {
    state: AppState,
    chat: sqlx::SqlitePool,
    uploader: String,
    room: i64,
}

async fn setup(stt: Arc<dyn lets_chat::stt::SttClient>) -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let a = db::auth::create_user(&auth, "alice", "h").await.unwrap();
    let b = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    // Promote one user so backfill_general_membership runs (harness gotcha).
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&a)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
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
        hub,
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
        stt_client: Some(stt),
        llm_client: None,
        embedding_client: None,
    };
    Setup {
        state,
        chat,
        uploader: b,
        room: 1, // General, from the backfill above.
    }
}

/// Insert a linked attachment owned by the uploader and return
/// `(upload_id, message_id)`. `waveform` present + an `audio/*` mime is what
/// makes a row a voice message; a `video/*` mime with no waveform is a clip.
async fn seed(s: &Setup, rel: &str, mime: &str, waveform: Option<&str>) -> (i64, i64) {
    std::fs::write(db::uploads_dir().join(rel), b"fake-media-bytes").unwrap();
    let upload_id = db::uploads::insert_upload(&s.chat, &s.uploader, rel, mime, 16, rel, waveform)
        .await
        .unwrap();
    let mid = db::chat::insert_message(&s.chat, s.room, &s.uploader, "media")
        .await
        .unwrap();
    db::uploads::link_upload_to_message(&s.chat, upload_id, mid)
        .await
        .unwrap();
    (upload_id, mid)
}

const VOICE_WAVEFORM: &str = r#"{"d":1.5,"p":[0.1,0.4,0.2]}"#;

async fn transcript_of(s: &Setup, upload_id: i64) -> Option<String> {
    db::uploads::get_upload(&s.chat, upload_id)
        .await
        .unwrap()
        .unwrap()
        .0
        .transcript
}

async fn status_of(s: &Setup, upload_id: i64) -> Option<String> {
    db::uploads::get_upload(&s.chat, upload_id)
        .await
        .unwrap()
        .unwrap()
        .0
        .transcript_status
}

/// `LETS_CHAT_STT_SCOPE=voice` transcribes voice notes and skips the expensive
/// video-clip path - the whole point of the setting.
#[tokio::test]
async fn scope_voice_skips_clips_but_keeps_voice_notes() {
    let _guard = ENV_LOCK.lock().await;
    let s = setup(Arc::new(lets_chat::stt::MockSttClient::text("heard it"))).await;
    let (voice_id, voice_mid) = seed(&s, "lc592-v1.webm", "audio/webm", Some(VOICE_WAVEFORM)).await;
    let (clip_id, clip_mid) = seed(&s, "lc592-c1.webm", "video/webm", None).await;

    // SAFETY: the whole test holds ENV_LOCK; the var is removed at the end.
    unsafe { std::env::set_var("LETS_CHAT_STT_SCOPE", "voice") };
    routes::maybe_transcribe_voice_message(&s.state, voice_id, voice_mid, s.room)
        .await
        .unwrap();
    routes::maybe_transcribe_voice_message(&s.state, clip_id, clip_mid, s.room)
        .await
        .unwrap();
    unsafe { std::env::remove_var("LETS_CHAT_STT_SCOPE") };

    assert_eq!(
        transcript_of(&s, voice_id).await.as_deref(),
        Some("heard it")
    );
    assert_eq!(transcript_of(&s, clip_id).await, None, "clip was gated out");
    assert_eq!(
        status_of(&s, clip_id).await,
        None,
        "a gated-out clip is policy, not failure: no Retry control for something \
         the operator switched off"
    );
}

/// `clips` is the mirror image, and `none` gates both. Proves the switch is a
/// real four-way choice rather than an on/off flag with extra words.
#[tokio::test]
async fn scope_clips_and_none_gate_the_other_directions() {
    let _guard = ENV_LOCK.lock().await;
    let s = setup(Arc::new(lets_chat::stt::MockSttClient::text("heard it"))).await;
    let (voice_id, voice_mid) = seed(&s, "lc592-v2.webm", "audio/webm", Some(VOICE_WAVEFORM)).await;
    let (clip_id, clip_mid) = seed(&s, "lc592-c2.webm", "video/webm", None).await;

    // SAFETY: the whole test holds ENV_LOCK; the var is removed at the end.
    unsafe { std::env::set_var("LETS_CHAT_STT_SCOPE", "clips") };
    routes::maybe_transcribe_voice_message(&s.state, voice_id, voice_mid, s.room)
        .await
        .unwrap();
    routes::maybe_transcribe_voice_message(&s.state, clip_id, clip_mid, s.room)
        .await
        .unwrap();
    assert_eq!(transcript_of(&s, voice_id).await, None, "voice gated out");
    assert_eq!(
        transcript_of(&s, clip_id).await.as_deref(),
        Some("heard it")
    );

    let (v2, v2m) = seed(&s, "lc592-v3.webm", "audio/webm", Some(VOICE_WAVEFORM)).await;
    let (c2, c2m) = seed(&s, "lc592-c3.webm", "video/webm", None).await;
    unsafe { std::env::set_var("LETS_CHAT_STT_SCOPE", "none") };
    routes::maybe_transcribe_voice_message(&s.state, v2, v2m, s.room)
        .await
        .unwrap();
    routes::maybe_transcribe_voice_message(&s.state, c2, c2m, s.room)
        .await
        .unwrap();
    unsafe { std::env::remove_var("LETS_CHAT_STT_SCOPE") };

    assert_eq!(transcript_of(&s, v2).await, None);
    assert_eq!(transcript_of(&s, c2).await, None);
}

/// With no scope set, both kinds transcribe - the pre-LC-592 behaviour is the
/// default, so upgrading does not silently switch anything off.
#[tokio::test]
async fn default_scope_transcribes_both_kinds() {
    let _guard = ENV_LOCK.lock().await;
    // SAFETY: the whole test holds ENV_LOCK.
    unsafe { std::env::remove_var("LETS_CHAT_STT_SCOPE") };
    let s = setup(Arc::new(lets_chat::stt::MockSttClient::text("heard it"))).await;
    let (voice_id, voice_mid) = seed(&s, "lc592-v4.webm", "audio/webm", Some(VOICE_WAVEFORM)).await;
    let (clip_id, clip_mid) = seed(&s, "lc592-c4.webm", "video/webm", None).await;

    routes::maybe_transcribe_voice_message(&s.state, voice_id, voice_mid, s.room)
        .await
        .unwrap();
    routes::maybe_transcribe_voice_message(&s.state, clip_id, clip_mid, s.room)
        .await
        .unwrap();

    assert_eq!(
        transcript_of(&s, voice_id).await.as_deref(),
        Some("heard it")
    );
    assert_eq!(
        transcript_of(&s, clip_id).await.as_deref(),
        Some("heard it")
    );
}

/// Submissions past the per-room cap are refused, and refused VISIBLY: the
/// upload is marked failed so its author gets the LC-590 Retry control once the
/// window rolls over, rather than the transcript quietly never appearing.
#[tokio::test]
async fn over_cap_submissions_are_refused_and_marked_failed() {
    let _guard = ENV_LOCK.lock().await;
    let s = setup(Arc::new(lets_chat::stt::MockSttClient::text("heard it"))).await;
    let cap = lets_chat::stt_load::DEFAULT_ROOM_PER_MINUTE as usize;

    let mut ids = Vec::new();
    for i in 0..cap {
        let (id, mid) = seed(
            &s,
            &format!("lc592-cap-{i}.webm"),
            "audio/webm",
            Some(VOICE_WAVEFORM),
        )
        .await;
        routes::maybe_transcribe_voice_message(&s.state, id, mid, s.room)
            .await
            .unwrap();
        ids.push(id);
    }
    for id in &ids {
        assert_eq!(
            transcript_of(&s, *id).await.as_deref(),
            Some("heard it"),
            "everything up to the cap goes through"
        );
    }

    // One past the cap.
    let (over_id, over_mid) = seed(
        &s,
        "lc592-cap-over.webm",
        "audio/webm",
        Some(VOICE_WAVEFORM),
    )
    .await;
    routes::maybe_transcribe_voice_message(&s.state, over_id, over_mid, s.room)
        .await
        .unwrap();
    assert_eq!(transcript_of(&s, over_id).await, None, "refused");
    assert_eq!(
        status_of(&s, over_id).await.as_deref(),
        Some("failed"),
        "refused work must be visible and retryable, not silently dropped"
    );
}

/// A burst is bounded AND drains. Two distinct claims, both needed: the
/// high-water mark proves the limiter actually caps concurrent engine calls, and
/// every job completing proves permits are RELEASED - a leaked permit would
/// deadlock the run after `LETS_CHAT_STT_WORKERS` jobs.
#[tokio::test]
async fn concurrent_burst_is_bounded_and_drains() {
    let _guard = ENV_LOCK.lock().await;
    // A SLOW mock, so concurrent calls genuinely overlap. With an instant mock
    // the concurrency assertion below would pass even with no limiter at all.
    let mock = Arc::new(lets_chat::stt::MockSttClient::slow("heard it", 40));
    let s = setup(mock.clone()).await;
    // Comfortably above the default 2 permits, comfortably under the room cap.
    let burst = 8usize;
    assert!(
        burst > lets_chat::stt_load::DEFAULT_STT_WORKERS,
        "must queue"
    );
    assert!(burst <= lets_chat::stt_load::DEFAULT_ROOM_PER_MINUTE as usize);

    let mut seeded = Vec::new();
    for i in 0..burst {
        seeded.push(
            seed(
                &s,
                &format!("lc592-burst-{i}.webm"),
                "audio/webm",
                Some(VOICE_WAVEFORM),
            )
            .await,
        );
    }

    let mut handles = Vec::new();
    for (id, mid) in seeded.clone() {
        let state = s.state.clone();
        let room = s.room;
        handles.push(tokio::spawn(async move {
            routes::maybe_transcribe_voice_message(&state, id, mid, room).await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }

    for (id, _) in seeded {
        assert_eq!(
            transcript_of(&s, id).await.as_deref(),
            Some("heard it"),
            "the burst drained; nothing was dropped by the limiter"
        );
    }
    assert_eq!(
        mock.call_count(),
        burst,
        "every job reached the engine once"
    );
    assert!(
        mock.max_concurrent() <= lets_chat::stt_load::DEFAULT_STT_WORKERS,
        "never more than {} engine calls at once, saw {}",
        lets_chat::stt_load::DEFAULT_STT_WORKERS,
        mock.max_concurrent()
    );
}
