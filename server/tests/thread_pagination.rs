//! LC-797: the thread panel renders ONE page of replies, not the whole thread.
//!
//! Before this the panel read every reply under a parent (`list_thread_replies`,
//! no `LIMIT`) and built a `MessageView` for each, so rows read, markdown
//! renders, bytes emitted and DOM nodes all grew linearly in the thread's
//! length - the exact shape LC-779 bounded for the room timeline and left the
//! panel out of. These tests pin the page bound, the sentinel chain that keeps
//! older replies reachable, and the fact that a page boundary is invisible in
//! the rendered rows.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use lets_chat::{db, routes, state::AppState, ws::hub::Hub};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

mod common;

fn ensure_tempdir() {
    static TEMPDIR: OnceLock<String> = OnceLock::new();
    TEMPDIR.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-thread-pagination-{}", std::process::id()));
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
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

struct Setup {
    app: Router,
    session: String,
    room: i64,
    parent: i64,
    /// Reply ids in insertion order (oldest first).
    replies: Vec<i64>,
    /// LC-806: the chat pool, so a test can seed replies by a second author or
    /// move a reply to another day.
    chat: sqlx::SqlitePool,
    /// LC-806: a second user, for author-change tests.
    other_id: String,
}

/// Seed a thread with `reply_count` replies under one root.
async fn setup(reply_count: usize) -> Setup {
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
    let other_id = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();

    let room = 1; // General, from the backfill.
    let parent = db::chat::insert_message(&chat, room, &admin_id, "thread root")
        .await
        .unwrap();
    let mut replies = Vec::with_capacity(reply_count);
    for i in 0..reply_count {
        replies.push(
            db::chat::insert_reply(&chat, room, &admin_id, &format!("reply {i}"), parent)
                .await
                .unwrap(),
        );
    }

    let session = db::auth::create_session(&auth, &admin_id).await.unwrap();
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
    Setup {
        app: routes::build_router(state.clone()),
        session,
        room,
        parent,
        replies,
        chat,
        other_id,
    }
}

/// Ids of the reply rows in a fragment, in document order.
///
/// LC-806: an older-page fragment ends with ONE out-of-band re-render of the
/// row that was already on screen (the page boundary, `hx-swap-oob="outerHTML"`
/// on its wrapper). That row is not part of the page, so scanning stops at the
/// marker; every id before it is a row the page actually adds.
fn rendered_reply_ids(html: &str) -> Vec<i64> {
    let html = match html.find("hx-swap-oob=\"outerHTML\"") {
        Some(i) => &html[..i],
        None => html,
    };
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("id=\"threadmsg-") {
        rest = &rest[i + "id=\"threadmsg-".len()..];
        if let Some(end) = rest.find('"') {
            if let Ok(id) = rest[..end].parse::<i64>() {
                out.push(id);
            }
            rest = &rest[end + 1..];
        }
    }
    out
}

/// The `hx-get` URL of the load-older sentinel, if the fragment carries one.
/// Scans every `hx-get` because the panel is full of them (reaction pickers,
/// the emoji picker, the edit-history button).
fn sentinel_url(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(i) = rest.find("hx-get=\"") {
        rest = &rest[i + "hx-get=\"".len()..];
        let end = rest.find('"')?;
        let url = &rest[..end];
        if url.contains("/messages?before=") {
            return Some(url.to_string());
        }
        rest = &rest[end + 1..];
    }
    None
}

/// The rendered markup of one reply row, from its opening tag to the start of
/// the next row (or the end of the fragment), with surrounding whitespace
/// trimmed so a row compares equal wherever it sits in the page.
fn row_markup(html: &str, id: i64) -> String {
    let anchor = format!("<div id=\"threadmsg-{id}\"");
    let start = html
        .find(&anchor)
        .unwrap_or_else(|| panic!("reply {id} is not in this fragment"));
    let rest = &html[start..];
    let end = rest[anchor.len()..]
        .find("<div id=\"threadmsg-")
        .map(|n| n + anchor.len())
        .unwrap_or(rest.len());
    rest[..end].trim_end().to_string()
}

/// The panel renders at most one page, however long the thread is, and says so
/// with a sentinel. The count divider still names the WHOLE thread.
#[tokio::test]
async fn panel_renders_one_page_and_offers_a_sentinel() {
    let page = db::chat::THREAD_REPLY_PAGE_LIMIT as usize;
    let total = page * 2 + 7;
    let s = setup(total).await;

    let (st, panel) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}", s.room, s.parent),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let rendered = rendered_reply_ids(&panel);
    // The thread ROOT renders through the same partial, so it is in the list too.
    assert_eq!(
        rendered.len(),
        page + 1,
        "panel renders one page of replies plus the root, not the whole thread"
    );
    assert_eq!(
        &rendered[1..],
        &s.replies[total - page..],
        "the page is the NEWEST replies, oldest-first"
    );
    assert!(
        panel.contains(&format!("<span class=\"lc-thread-count-label\">{total} ")),
        "the count divider labels the whole thread, not the loaded page"
    );
    assert!(
        sentinel_url(&panel).is_some(),
        "older replies exist, so the panel offers a sentinel"
    );
}

/// A thread that fits in one page gets NO sentinel: the trigger fires on
/// intersection, so a stray sentinel would fetch an empty page on every open.
#[tokio::test]
async fn short_thread_has_no_sentinel() {
    let s = setup(3).await;

    let (st, panel) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}", s.room, s.parent),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rendered_reply_ids(&panel).len(), 4, "root + 3 replies");
    assert_eq!(
        sentinel_url(&panel),
        None,
        "nothing older exists, so no sentinel"
    );
}

/// AC: seed more than two pages, open the panel, follow the sentinel to the
/// oldest page, and the thread's FIRST reply is reachable. Each fetched page
/// carries the next sentinel until history is exhausted, and then stops.
#[tokio::test]
async fn following_the_sentinel_reaches_the_first_reply() {
    let page = db::chat::THREAD_REPLY_PAGE_LIMIT as usize;
    let total = page * 2 + 7; // three pages: 50, 50, 7
    let s = setup(total).await;

    let (st, panel) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}", s.room, s.parent),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let mut seen = rendered_reply_ids(&panel);
    seen.remove(0); // drop the root, which is not a reply
    let mut next = sentinel_url(&panel).expect("first page has a sentinel");
    let mut pages = 1;

    while pages < 10 {
        let (st, frag) = send(&s.app, &s.session, Method::GET, &next).await;
        assert_eq!(st, StatusCode::OK, "older page {pages} loads");
        pages += 1;
        let ids = rendered_reply_ids(&frag);
        assert!(!ids.is_empty(), "an offered page is never empty");
        // Older replies land ABOVE what is already rendered.
        let mut merged = ids.clone();
        merged.extend(seen.iter().copied());
        seen = merged;
        match sentinel_url(&frag) {
            Some(u) => next = u,
            None => break,
        }
    }

    assert_eq!(pages, 3, "50 + 50 + 7 replies is exactly three pages");
    assert_eq!(seen, s.replies, "every reply, in order, is reachable");
    assert_eq!(
        seen.first().copied(),
        s.replies.first().copied(),
        "the thread's first reply is on the last page"
    );
}

/// The last page carries no sentinel, so the chain terminates instead of
/// re-fetching the start of the thread forever.
#[tokio::test]
async fn oldest_page_carries_no_sentinel() {
    let page = db::chat::THREAD_REPLY_PAGE_LIMIT;
    let s = setup(page as usize + 5).await;

    // Ask directly for the page before the 6th-oldest reply: 5 replies remain.
    let cursor = s.replies[5];
    let (st, frag) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!(
            "/room/{}/thread/{}/messages?before={}",
            s.room, s.parent, cursor
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rendered_reply_ids(&frag), s.replies[..5]);
    assert_eq!(sentinel_url(&frag), None, "history is exhausted");
    assert!(
        (rendered_reply_ids(&frag).len() as i64) < page,
        "a short page means there is nothing behind it"
    );
}

// ── LC-806: the panel groups like the timeline ─────────────────────────────
// `is_follow_up` is computed against the row above with the timeline's own
// predicate (`db::chat::is_follow_up_of`), a UTC day change renders the day
// separator, and - because a row now depends on its predecessor - the
// load-older fragment re-renders the on-screen boundary row out of band. These
// replace `thread_page_boundary_row_is_identical`, which pinned the OLD
// row-independence and existed to fail the moment grouping arrived without
// that re-render.

/// The wrapper (`threadrow-{id}`: optional day separator + the row) of one
/// reply, up to the next wrapper. Where a day label lives, so tests about
/// separators read this rather than `row_markup`.
fn wrapper_markup(html: &str, id: i64) -> String {
    let anchor = format!("<div id=\"threadrow-{id}\"");
    let start = html
        .find(&anchor)
        .unwrap_or_else(|| panic!("reply {id} wrapper is not in this fragment"));
    let rest = &html[start..];
    let end = rest[anchor.len()..]
        .find("<div id=\"threadrow-")
        .map(|n| n + anchor.len())
        .unwrap_or(rest.len());
    rest[..end].trim_end().to_string()
}

const FOLLOW_UP_MARK: &str = "lc-followup-ts";
const DAY_MARK: &str = "lc-day-chip";

/// Consecutive replies by one author inside the grouping window collapse into
/// a follow-up run: the page's first reply is a header (avatar + author), the
/// rest drop the header and keep only the hover HH:MM, as in the timeline.
#[tokio::test]
async fn same_author_run_groups_into_follow_ups() {
    let s = setup(3).await;
    let (st, panel) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}", s.room, s.parent),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let head = row_markup(&panel, s.replies[0]);
    assert!(
        !head.contains(FOLLOW_UP_MARK) && head.contains("font-semibold"),
        "the page's first reply renders as a header: {head}"
    );
    for &id in &s.replies[1..] {
        let row = row_markup(&panel, id);
        assert!(
            row.contains(FOLLOW_UP_MARK) && !row.contains("font-semibold"),
            "reply {id} groups under the same-author row above it: {row}"
        );
    }
}

/// A different author starts a fresh header; a UTC day change starts a fresh
/// header AND renders the day separator above the row. The page's first reply
/// carries the day label (the timeline's rule for a page start); a same-day
/// successor does not.
#[tokio::test]
async fn author_change_and_day_change_break_the_run() {
    let s = setup(2).await;
    let by_other = db::chat::insert_reply(&s.chat, s.room, &s.other_id, "from bob", s.parent)
        .await
        .unwrap();
    let next_day = db::chat::insert_reply(&s.chat, s.room, &s.other_id, "tomorrow", s.parent)
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET created_at = datetime(created_at, '+1 day') WHERE id = ?")
        .bind(next_day)
        .execute(&s.chat)
        .await
        .unwrap();

    let (st, panel) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}", s.room, s.parent),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    assert!(
        wrapper_markup(&panel, s.replies[0]).contains(DAY_MARK),
        "the page's first reply opens with the day separator"
    );
    let second = wrapper_markup(&panel, s.replies[1]);
    assert!(
        second.contains(FOLLOW_UP_MARK) && !second.contains(DAY_MARK),
        "same author, same day: grouped, no separator: {second}"
    );
    let bob = wrapper_markup(&panel, by_other);
    assert!(
        !bob.contains(FOLLOW_UP_MARK) && !bob.contains(DAY_MARK),
        "a different author breaks the run without a day change: {bob}"
    );
    let tomorrow = wrapper_markup(&panel, next_day);
    assert!(
        tomorrow.contains(DAY_MARK) && !tomorrow.contains(FOLLOW_UP_MARK),
        "a day change renders the separator and a fresh header, even for the \
         same author within the window: {tomorrow}"
    );
}

/// The page boundary falls inside a same-author run. On the first page the
/// boundary reply is the page's first row, so it renders as a header with a
/// day label; when the older page arrives its last reply becomes that row's
/// predecessor, and the fragment re-renders the row out of band as a follow-up
/// with the day label gone. Exactly what the timeline's older-page fragment
/// does; without it the join would show a spurious header + separator.
#[tokio::test]
async fn older_page_rerenders_the_boundary_row_out_of_band() {
    let page = db::chat::THREAD_REPLY_PAGE_LIMIT as usize;
    let s = setup(page + 10).await;
    // The oldest row of the first (newest) page.
    let boundary = s.replies[10];

    let (st, panel) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}", s.room, s.parent),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let before = wrapper_markup(&panel, boundary);
    assert!(
        !before.contains(FOLLOW_UP_MARK) && before.contains(DAY_MARK),
        "as a page start the boundary row is a header with a day label: {before}"
    );

    let (st, older) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!(
            "/room/{}/thread/{}/messages?before={}",
            s.room, s.parent, boundary
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    // The page's own rows swap in place of the sentinel; the boundary row is
    // the ONE out-of-band swap, and it now groups under the page's last reply.
    assert_eq!(
        older.matches("hx-swap-oob=\"outerHTML\"").count(),
        1,
        "exactly one out-of-band re-render, the boundary row"
    );
    let after = wrapper_markup(&older, boundary);
    assert!(
        after.contains("hx-swap-oob=\"outerHTML\""),
        "the boundary row is re-rendered out of band: {after}"
    );
    assert!(
        after.contains(FOLLOW_UP_MARK) && !after.contains(DAY_MARK),
        "against its new predecessor the boundary row is a follow-up with no \
         day label: {after}"
    );
    // The fetched page itself groups internally: first row header, rest follow-ups.
    assert!(!row_markup(&older, s.replies[0]).contains(FOLLOW_UP_MARK));
    assert!(row_markup(&older, s.replies[9]).contains(FOLLOW_UP_MARK));
}

/// The panel's response size is constant in the thread's length: a 500-reply
/// thread costs what a one-page thread costs. Size rather than wall-clock,
/// which is the same measure LC-779 used for the timeline and the only one that
/// is stable under a loaded CI host.
///
/// The comparison baseline is one FULL page (51 replies), not the 20 named in
/// LC-797: a 20-reply thread is shorter than a page, so it renders 20 rows by
/// definition and can never be within 20 percent of a 50-row page. What the
/// criterion actually encodes - the panel no longer grows with the thread - is
/// what is asserted here.
#[tokio::test]
async fn panel_size_is_constant_in_thread_length() {
    let page = db::chat::THREAD_REPLY_PAGE_LIMIT as usize;

    let mut sizes = Vec::new();
    for total in [page + 1, page * 10] {
        let s = setup(total).await;
        let (st, panel) = send(
            &s.app,
            &s.session,
            Method::GET,
            &format!("/room/{}/thread/{}", s.room, s.parent),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            rendered_reply_ids(&panel).len(),
            page + 1,
            "{total}-reply thread still renders one page plus the root"
        );
        sizes.push(panel.len());
    }

    let (small, large) = (sizes[0] as f64, sizes[1] as f64);
    let growth = (large - small).abs() / small;
    assert!(
        growth < 0.20,
        "a {}-reply thread renders {large} bytes vs {small} for a {}-reply \
         thread ({:.1} percent apart); the panel must not grow with the thread",
        page * 10,
        page + 1,
        growth * 100.0
    );
}

/// The unbounded `list_thread_replies` has no caller on a render path. The two
/// that remain are the LLM digest paths, which must read the whole thread.
///
/// A source-level guard because there is no runtime signal for "this call was
/// on a render path": a re-introduced unbounded read renders correctly, it just
/// costs the whole thread. It stays green for any caller in the allowed set and
/// fails the moment a new file starts calling it.
#[test]
fn unbounded_thread_read_has_no_render_caller() {
    /// Every `.rs` file under `dir`, recursively.
    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);

    let mut callers: Vec<String> = files
        .iter()
        .filter(|p| {
            // The `(` excludes `list_thread_replies_page` / `_paginated`.
            std::fs::read_to_string(p)
                .expect("read source")
                .contains("list_thread_replies(")
        })
        .map(|p| {
            p.strip_prefix(&src)
                .expect("under src")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    callers.sort();

    assert_eq!(
        callers,
        vec![
            // The definition itself.
            "db/chat.rs".to_string(),
            // LC-484 thread summary: a digest, not a render.
            "routes/summary.rs".to_string(),
            // LC-668 thread title: a digest, not a render.
            "routes/thread_title.rs".to_string(),
        ],
        "LC-797: the thread panel renders one page via list_thread_replies_page. \
         A new caller of the unbounded list_thread_replies is a render path \
         regression unless it is another whole-thread digest - if it is, add it \
         to this list with a comment saying why."
    );
}

/// The load-older endpoint is gated exactly like the panel it feeds.
#[tokio::test]
async fn older_replies_endpoint_is_access_gated() {
    let s = setup(5).await;

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/room/{}/thread/{}/messages?before={}",
            s.room, s.parent, s.replies[4]
        ))
        .body(Body::empty())
        .unwrap();
    let resp = s.app.clone().oneshot(req).await.unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "an unauthenticated request does not get thread history"
    );

    // A parent that is itself a reply is not a thread root.
    let (st, _) = send(
        &s.app,
        &s.session,
        Method::GET,
        &format!("/room/{}/thread/{}/messages", s.room, s.replies[0]),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}
