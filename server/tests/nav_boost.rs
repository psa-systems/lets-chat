//! LC-837: boosted navigation, the server half.
//!
//! Links in the nav panel carry `hx-boost` targeting `#main` (rendered from
//! `partials/nav_boost.html`), so a boosted GET carries `HX-Request: true` like
//! any htmx request, plus `HX-Boosted: true`; a back/forward restore carries
//! `HX-History-Restore-Request: true`. Both need the WHOLE page, because the
//! client selects `#main` out of it. Every GET path that used to treat a bare
//! `hx-request` as "send a fragment" therefore goes through
//! `routes::wants_fragment`, which these tests pin from the outside: the
//! transcripts index (the one GET handler with a fragment branch) and the
//! branding middleware (which skipped injection for every htmx request).
//!
//! The template side is pinned too: each nav anchor is boosted per anchor or
//! explicitly opted out, and `#main` is the history element, so a restore never
//! touches the `ws-connect` wrapper.
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::push::{MockPushClient, PushClient};
use lets_chat::ws::hub::Hub;
use lets_chat::{db, routes, state::AppState};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

struct App {
    app: Router,
    session: String,
    room_id: i64,
}

async fn setup_app() -> App {
    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let user_id = db::auth::create_user(&auth, "navigator", "hash")
        .await
        .unwrap();
    let session = db::auth::create_session(&auth, &user_id).await.unwrap();
    let enclave_id = db::enclave::create_enclave(&chat, "Team", None, &user_id)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat, "general", None, "public", None, Some(enclave_id))
        .await
        .unwrap();

    let hub = Arc::new(Hub::new());
    let bg = lets_chat::bg::spawn(auth.clone());
    let state = AppState {
        geoip: None,
        login_approval_enabled: false,
        auth,
        chat,
        settings,
        hub,
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new([0u8; 32])),
        vapid: None,
        push_client: Arc::new(MockPushClient::default()) as Arc<dyn PushClient>,
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
    App {
        app: routes::build_router(state),
        session,
        room_id,
    }
}

/// GET `path` as the logged-in user with the given extra headers.
async fn get(app: &App, path: &str, extra: &[(&str, &str)]) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::COOKIE, format!("session={}", app.session));
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let res = app
        .app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

const MAIN: &str = "<main id=\"main\" hx-history-elt";
const BRAND: &str = "<style data-lc-brand>";
const BOOST: &str =
    "hx-boost=\"true\" hx-target=\"#main\" hx-select=\"#main\" hx-swap=\"outerHTML\"";

// ---------------------------------------------------------------------------
// `wants_fragment`: what a fragment GET is, and what it is not.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_boosted_get_renders_the_whole_page_where_a_fragment_get_does_not() {
    let a = setup_app().await;

    // The LC-445 search/filter path: a bare htmx GET still gets the list body.
    let (status, fragment) = get(&a, "/transcripts", &[("HX-Request", "true")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !fragment.contains(MAIN),
        "an htmx filter request gets the list body, not the page, got:\n{fragment}"
    );

    // A boosted navigation to the same URL is a page load.
    let (status, page) = get(
        &a,
        "/transcripts",
        &[("HX-Request", "true"), ("HX-Boosted", "true")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains(MAIN),
        "a boosted navigation must render the whole page (the client selects #main out of it), got:\n{page}"
    );
    assert!(
        page.contains("<title>"),
        "the page carries a <title> for htmx to apply, got:\n{page}"
    );
}

#[tokio::test]
async fn a_history_restore_renders_the_whole_page() {
    let a = setup_app().await;
    let (status, page) = get(
        &a,
        "/transcripts",
        &[
            ("HX-Request", "true"),
            ("HX-History-Restore-Request", "true"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains(MAIN),
        "back/forward re-fetch the page and htmx swaps its hx-history-elt into #main; a fragment here leaves the old page in place, got:\n{page}"
    );
}

#[tokio::test]
async fn branding_css_is_injected_into_a_boosted_page_and_not_into_a_fragment() {
    let a = setup_app().await;
    let room = format!("/room/{}", a.room_id);

    let (_, full) = get(&a, &room, &[]).await;
    assert!(
        full.contains(BRAND),
        "a full load carries the branding style"
    );

    let (_, boosted) = get(&a, &room, &[("HX-Request", "true"), ("HX-Boosted", "true")]).await;
    assert!(
        boosted.contains(BRAND),
        "a boosted page carries the branding style for nav.js to move into <head>, got:\n{boosted}"
    );

    let (_, fragment) = get(&a, "/transcripts", &[("HX-Request", "true")]).await;
    assert!(
        !fragment.contains(BRAND),
        "the fragment fast path is unchanged: no <head>, nothing injected"
    );
}

// ---------------------------------------------------------------------------
// The template side.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nav_links_are_boosted_per_anchor_and_the_exemptions_are_real_navigations() {
    let a = setup_app().await;
    let (status, page) = get(&a, &format!("/room/{}", a.room_id), &[]).await;
    assert_eq!(status, StatusCode::OK);

    // The room row, a DM-shape link and the account menu's plain settings link
    // are boosted, each on its own anchor.
    for href in [
        format!("/room/{}", a.room_id),
        "/inbox".to_string(),
        "/settings".to_string(),
    ] {
        let needle = format!("href=\"{href}\" {BOOST}");
        assert!(
            page.contains(&needle),
            "expected a boosted anchor `{needle}`, got:\n{page}"
        );
    }

    // The exemptions: logout lands on a page with no #main; the settings deep
    // links depend on location.hash while the page's scripts run.
    for href in ["/logout", "/settings#profile"] {
        let needle = format!("href=\"{href}\" hx-boost=\"false\"");
        assert!(
            page.contains(&needle),
            "expected a real navigation `{needle}`, got:\n{page}"
        );
    }

    // Boost is never on a container: the only hx-boost="true" on the page are
    // the per-anchor ones, and each sits on an <a>.
    let container_boosts = page
        .match_indices("hx-boost=\"true\"")
        .filter(|(i, _)| {
            let before = &page[..*i];
            let tag_start = before.rfind('<').unwrap_or(0);
            !before[tag_start..].starts_with("<a ")
        })
        .count();
    assert_eq!(
        container_boosts, 0,
        "hx-boost=\"true\" found outside an <a> opening tag"
    );
}

#[tokio::test]
async fn main_is_the_history_element_so_a_restore_never_touches_the_socket() {
    let a = setup_app().await;
    let (_, page) = get(&a, &format!("/room/{}", a.room_id), &[]).await;
    assert!(page.contains(MAIN), "got:\n{page}");
    // Exactly one history element, and it is #main, not the ws-connect wrapper.
    assert_eq!(page.matches("hx-history-elt").count(), 1);
    let ws = page
        .find("ws-connect=\"/ws\"")
        .expect("the socket wrapper renders");
    let main = page.find(MAIN).unwrap();
    assert!(
        ws < main,
        "the ws-connect wrapper encloses #main, not the other way round"
    );
}
