//! LC-595: reactions on thread replies, and the id namespacing that makes them
//! possible.
//!
//! The thread panel renders the same messages the timeline does (the thread ROOT
//! appears in both), so before this the panel either had no reaction bar at all
//! or, had one been added naively, would have duplicated `#reactions-{id}` -
//! which is the LC-553 bug where the first `outerHTML` toggle replaced the wrong
//! node. These tests pin the namespace, the live update reaching both surfaces,
//! and the absence of any id collision between the two.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-thread-reactions-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create test data dir");
        db::set_data_dir(p.to_string_lossy().to_string());
        p.to_string_lossy().to_string()
    });
}

async fn send(app: &Router, sess: &str, method: Method, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("session={sess}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Every `id="..."` in a fragment, in document order (duplicates preserved, so a
/// collision inside one document is detectable).
fn ids(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("id=\"") {
        rest = &rest[i + 4..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
    }
    out
}

struct Setup {
    app: Router,
    state: AppState,
    session: String,
    user_id: String,
    room: i64,
    parent: i64,
    reply: i64,
}

async fn setup() -> Setup {
    ensure_tempdir();
    let auth = common::pool("auth").await;
    let chat = common::pool("chat").await;
    let settings = common::pool("settings").await;

    let admin_id = db::auth::create_user(&auth, "admin", "h").await.unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&admin_id)
        .execute(&auth)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let room = 1; // General, from the backfill.
    let parent = db::chat::insert_message(&chat, room, &admin_id, "thread root")
        .await
        .unwrap();
    let reply = db::chat::insert_reply(&chat, room, &admin_id, "a reply", parent)
        .await
        .unwrap();

    let session = db::auth::create_session(&auth, &admin_id).await.unwrap();
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
    Setup {
        app: routes::build_router(state.clone()),
        state,
        session,
        user_id: admin_id,
        room,
        parent,
        reply,
    }
}

/// A thread reply can be reacted to, and the reaction shows up in the panel.
/// Pre-LC-595 the panel had no bar at all, so this was impossible in both
/// directions: you could not add one, and an existing one was invisible.
#[tokio::test]
async fn thread_reply_can_be_reacted_to_and_shows_its_reactions() {
    let s = setup().await;

    let (st, body) = send(
        &s.app,
        &s.session,
        Method::POST,
        &format!(
            "/messages/{}/reactions/%F0%9F%91%8D?surface=thread",
            s.reply
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body.contains(&format!("id=\"thread-reactions-{}\"", s.reply)),
        "the toggle answers in the CALLER's namespace, else the swap would \
         inject a duplicate timeline id into the panel; got: {body}"
    );
    assert!(
        !body.contains(&format!("id=\"reactions-{}\"", s.reply)),
        "must not carry the timeline id"
    );

    // ...and the panel renders it on the next open.
    let (st, panel) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}", s.room, s.parent),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        panel.contains(&format!("id=\"thread-reactions-{}\"", s.reply)),
        "the reply's bar is present"
    );
    assert!(
        panel.contains("\u{1f44d}"),
        "the stored reaction is actually rendered, not just an empty bar"
    );
}

/// The chips and picker inside the panel keep the thread namespace in BOTH their
/// URL and their target. Miss either and the reaction round-trips back into the
/// timeline's id.
#[tokio::test]
async fn thread_controls_carry_the_surface_in_url_and_target() {
    let s = setup().await;
    send(
        &s.app,
        &s.session,
        Method::POST,
        &format!(
            "/messages/{}/reactions/%F0%9F%91%8D?surface=thread",
            s.reply
        ),
    )
    .await;
    let (_, panel) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}", s.room, s.parent),
    )
    .await;

    assert!(
        panel.contains(&format!("hx-target=\"#thread-reactions-{}\"", s.reply)),
        "chip targets the thread bar"
    );
    assert!(
        panel.contains("?surface=thread"),
        "chip + picker URLs carry the surface"
    );

    // The picker opened from the thread must react back into the thread too.
    let (st, picker) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/messages/{}/reactions/picker?surface=thread", s.reply),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        picker.contains(&format!("hx-target=\"#thread-reactions-{}\"", s.reply)),
        "picker buttons target the thread bar"
    );
    assert!(
        !picker.contains(&format!("hx-target=\"#reactions-{}\"", s.reply)),
        "no button targets the timeline bar"
    );

    // Without the parameter the picker is byte-for-byte the pre-LC-595 timeline
    // one, so nothing that already opens it had to change.
    let (_, timeline_picker) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/messages/{}/reactions/picker", s.reply),
    )
    .await;
    assert!(timeline_picker.contains(&format!("hx-target=\"#reactions-{}\"", s.reply)));
    assert!(!timeline_picker.contains("?surface=thread"));
}

/// The live OOB update addresses BOTH surfaces. A reaction made in the timeline
/// has to reach an open thread panel and vice versa; before this the broadcast
/// only ever named `#reactions-{id}`, so it was a silent no-op for any reply
/// shown in a panel.
#[tokio::test]
async fn live_update_targets_both_the_timeline_and_the_thread() {
    let s = setup().await;
    db::chat::toggle_reaction(&s.state.chat, s.reply, &s.user_id, "\u{1f44d}")
        .await
        .unwrap();

    let payload = routes::test_support::render_reaction_bar(&s.state, s.reply, &s.user_id)
        .await
        .expect("renders");

    assert!(
        payload.contains(&format!("id=\"reactions-{}\"", s.reply)),
        "timeline copy present"
    );
    assert!(
        payload.contains(&format!("id=\"thread-reactions-{}\"", s.reply)),
        "thread copy present"
    );
    assert_eq!(
        payload.matches("hx-swap-oob=\"outerHTML\"").count(),
        2,
        "both copies are out-of-band swaps, not one wrapping the other"
    );
}

/// No id is shared between the room page and an open thread panel. This is the
/// invariant that keeps LC-553 fixed, and it covers the thread ROOT, which
/// appears in both surfaces and whose row id was duplicated before LC-595.
#[tokio::test]
async fn thread_panel_shares_no_id_with_the_timeline() {
    let s = setup().await;
    // Give the root and the reply reactions, so every bar actually renders.
    for id in [s.parent, s.reply] {
        db::chat::toggle_reaction(&s.state.chat, id, &s.user_id, "\u{1f44d}")
            .await
            .unwrap();
    }

    let (st, page) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}", s.room),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, panel) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}", s.room, s.parent),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Sanity: the root really is rendered in both places, or the assertion below
    // would pass by simply not overlapping.
    assert!(page.contains(&format!("id=\"msg-{}\"", s.parent)));
    assert!(panel.contains(&format!("id=\"threadmsg-{}\"", s.parent)));

    let panel_ids = ids(&panel);
    let mut seen = HashSet::new();
    let dupes: Vec<&String> = panel_ids.iter().filter(|id| !seen.insert(*id)).collect();
    assert!(dupes.is_empty(), "panel duplicates its own ids: {dupes:?}");

    // The panel REPLACES the page's #thread-panel placeholder (hx-swap
    // outerHTML), so that one id is expected to appear in both and is the only
    // legitimate overlap.
    let page_ids: HashSet<String> = ids(&page).into_iter().collect();
    let overlap: Vec<&String> = panel_ids
        .iter()
        .filter(|id| page_ids.contains(*id))
        .collect();
    assert_eq!(
        overlap,
        vec![&"thread-panel".to_string()],
        "the only id shared with the page may be the swap target itself"
    );
}
