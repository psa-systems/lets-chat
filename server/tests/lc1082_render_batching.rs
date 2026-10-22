//! LC-1082: finish the render-context migration for the unread badge and the
//! admin renderers. Before this, `render_unread_badge` re-ran
//! `list_blocked_ids_either_way` / `get_unread_count` once per background
//! recipient of a message, and `render_admin_reports` / `render_admin_support`
//! / `render_admin_voice_log` re-ran their backing list query and re-rendered
//! the same OOB fragment once per connected admin, inside each admin
//! connection's own receive loop. These tests pin: a message to N background
//! recipients resolves the blocked-ids and unread-count queries exactly once
//! (in the producer), not once per recipient; and an admin-topic broadcast
//! resolves its backing list query exactly once (in the publisher), not once
//! per subscribed admin connection, with every connection receiving the same
//! pre-rendered fragment.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use lets_chat::models::{Message, User};
use lets_chat::routes::render_context::NewMessageContext;
use lets_chat::ws::events::ChatEvent;
use lets_chat::ws::hub::Hub;
use lets_chat::{db, routes};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

mod common;

/// Collects the text of every statement sqlx logs on the `sqlx::query` target.
#[derive(Clone, Default)]
struct StatementLog(Arc<Mutex<Vec<String>>>);

struct StatementVisitor(Option<String>);

impl Visit for StatementVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "db.statement" {
            self.0 = Some(value.trim().to_string());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "db.statement" && self.0.is_none() {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for StatementLog {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut visitor = StatementVisitor(None);
        event.record(&mut visitor);
        if let Some(statement) = visitor.0 {
            self.0.lock().unwrap().push(statement);
        }
    }
}

impl StatementLog {
    /// The statements logged since the last call. A new pool connection's
    /// `PRAGMA` setup is not a query the render chose to run, so it is dropped.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
            .into_iter()
            .filter(|s| !s.starts_with("PRAGMA"))
            .collect()
    }
}

/// sqlx logs a SQLite statement from the connection's worker thread, so a
/// thread-local subscriber would miss it: the capture is the process-wide default.
fn statement_log() -> StatementLog {
    static LOG: OnceLock<StatementLog> = OnceLock::new();
    LOG.get_or_init(|| {
        let log = StatementLog::default();
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(log.clone()))
            .expect("no other global tracing subscriber in this test binary");
        log
    })
    .clone()
}

/// The capture is process-wide, so the tests that read it run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Fixture {
    state: lets_chat::state::AppState,
    log: StatementLog,
    _serial: tokio::sync::MutexGuard<'static, ()>,
    room: lets_chat::models::Room,
    author: User,
    members: Vec<User>,
    message: Message,
}

/// The General enclave room (id 1) holding an author and `member_count`
/// ordinary (background, non-subscribed) members, with one message posted by
/// the author. Mirrors `lc780_broadcast_render_context.rs`'s fixture.
async fn setup(member_count: usize) -> Fixture {
    let serial = SERIAL.lock().await;
    let log = statement_log();

    let auth = common::auth_pool().await;
    let chat = common::chat_pool().await;
    let settings = common::settings_pool().await;

    let author_id = db::auth::create_user(&auth, "author", "h").await.unwrap();
    let moderator_id = db::auth::create_user(&auth, "moderator", "h")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(&moderator_id)
        .execute(&auth)
        .await
        .unwrap();
    for i in 0..member_count {
        db::auth::create_user(&auth, &format!("member{i}"), "h")
            .await
            .unwrap();
    }
    db::enclave::backfill_general_membership(&auth, &chat)
        .await
        .unwrap();
    let room_id = 1;
    let msg_id = db::chat::insert_message(&chat, room_id, &author_id, "hello")
        .await
        .unwrap();
    let raw = db::chat::get_message(&chat, msg_id).await.unwrap().unwrap();
    let message = Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: raw.user_id,
        author_name: "author".to_string(),
        body: raw.body,
        created_at: raw.created_at,
        edited_at: raw.edited_at,
        parent_id: raw.parent_id,
        quote_id: raw.quote_id,
        is_system: raw.is_system,
        webhook_id: raw.webhook_id,
        email_inbox_id: raw.email_inbox_id,
        bridge_id: raw.bridge_id,
        bridge_foreign_name: raw.bridge_foreign_name,
        bridge_kind: raw.bridge_kind,
        bridge_foreign_avatar: raw.bridge_foreign_avatar,
    };

    let load = |name: &'static str| {
        let auth = auth.clone();
        async move {
            let rec = db::auth::find_user_by_username(&auth, name)
                .await
                .unwrap()
                .expect("user exists");
            User::from(rec)
        }
    };
    let author = load("author").await;
    let mut members = Vec::new();
    for i in 0..member_count {
        let name: &'static str = Box::leak(format!("member{i}").into_boxed_str());
        members.push(load(name).await);
    }
    let room = db::chat::get_room(&chat, room_id).await.unwrap().unwrap();

    let bg = lets_chat::bg::spawn(auth.clone());
    let state = lets_chat::state::AppState {
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
    Fixture {
        state,
        log,
        _serial: serial,
        room,
        author,
        members,
        message,
    }
}

/// The old, per-viewer blocked-ids query's statement text (`list_blocked_ids_either_way`).
const OLD_BLOCKED_IDS_MARKER: &str = "AS other_id";
/// The new, batched blocked-ids query's statement text (`list_blocked_ids_for_many`).
const BATCHED_BLOCKED_IDS_MARKER: &str = "blocker_id, blocked_id FROM user_blocks";
/// The old, per-viewer unread-count query's statement text (`get_unread_count`).
/// Distinct from the batched version, which prefixes the select list with
/// `? AS user_id,` so this exact substring never appears in it.
const OLD_UNREAD_COUNT_MARKER: &str = "SELECT COUNT(m.id) AS unread";
/// The new, batched unread-count query's statement text (`get_unread_counts_for_members`).
const BATCHED_UNREAD_COUNT_MARKER: &str = "AS user_id, COUNT(m.id) AS unread";

#[tokio::test]
async fn unread_badge_resolves_once_for_all_background_recipients() {
    let f = setup(3).await;
    f.log.take();
    let ctx = NewMessageContext::load(&f.state, &f.message, &f.room)
        .await
        .unwrap();
    let producer = f.log.take();
    let blocked_batches = producer
        .iter()
        .filter(|s| s.contains(BATCHED_BLOCKED_IDS_MARKER))
        .count();
    let unread_batches = producer
        .iter()
        .filter(|s| s.contains(BATCHED_UNREAD_COUNT_MARKER))
        .count();
    assert_eq!(
        blocked_batches, 1,
        "the producer must resolve every member's blocked set in one batched query: {producer:#?}"
    );
    assert_eq!(
        unread_batches, 1,
        "the producer must resolve every member's unread count in one batched query: {producer:#?}"
    );

    // 3 background (not-subscribed, not-author) recipients of a message to an
    // unmuted room: `allows_unread_bump()` is true only for the default
    // (`MuteMode::None`) setting, which is what a freshly created member has.
    let subscribed = Arc::new(Mutex::new(HashSet::new()));
    for member in &f.members {
        f.log.take();
        let html = routes::render_new_message_or_bump(
            &f.state,
            &f.message,
            &ctx,
            None,
            member,
            &subscribed,
        )
        .await;
        let statements = f.log.take();
        assert!(html.is_some(), "{} gets the unread badge", member.username);
        assert!(
            statements
                .iter()
                .all(|s| !s.contains(OLD_BLOCKED_IDS_MARKER) && !s.contains(OLD_UNREAD_COUNT_MARKER)),
            "{}'s badge render re-queried blocked ids / unread count per recipient: {statements:#?}",
            member.username
        );
    }
}

#[tokio::test]
async fn unread_badge_render_cost_is_independent_of_recipient_count() {
    let f = setup(6).await;
    let ctx = NewMessageContext::load(&f.state, &f.message, &f.room)
        .await
        .unwrap();
    let subscribed = Arc::new(Mutex::new(HashSet::new()));

    let mut per_recipient = Vec::new();
    for member in &f.members {
        f.log.take();
        let html = routes::render_new_message_or_bump(
            &f.state,
            &f.message,
            &ctx,
            None,
            member,
            &subscribed,
        )
        .await;
        assert!(html.is_some());
        per_recipient.push(f.log.take().len());
    }
    assert!(
        per_recipient.windows(2).all(|w| w[0] == w[1]),
        "per-recipient statement counts differ: {per_recipient:?}"
    );
}

/// Register `count` admin connections subscribed to the `admin` topic.
/// Returns their receivers so a test can drain what they were sent.
fn connect_admins(hub: &Hub, count: usize) -> Vec<tokio::sync::broadcast::Receiver<ChatEvent>> {
    (0..count)
        .map(|i| {
            let (conn_id, rx, _) = hub.connect(&format!("admin{i}"), &format!("admin{i}"));
            hub.subscribe_topic(conn_id, "admin");
            rx
        })
        .collect()
}

/// The admin renderers are `#[cfg(feature = "standalone")]`: in a `saas`
/// build the publisher never queries the backing list at all (the broadcast
/// still fires, carrying an empty fragment), so the "exactly once" contract
/// only applies to the `standalone` build this issue's acceptance criteria
/// target.
#[cfg(feature = "standalone")]
const EXPECTED_ADMIN_LIST_QUERIES: usize = 1;
#[cfg(not(feature = "standalone"))]
const EXPECTED_ADMIN_LIST_QUERIES: usize = 0;

/// Every admin receiver gets the identical already-rendered fragment for
/// `AdminVoiceLogChanged`.
fn drain_voice_log(rx: &mut tokio::sync::broadcast::Receiver<ChatEvent>) -> Arc<str> {
    match rx.try_recv().expect("admin connection received an event") {
        ChatEvent::AdminVoiceLogChanged { rendered } => rendered,
        other => panic!("expected AdminVoiceLogChanged, got {other:?}"),
    }
}

#[tokio::test]
async fn admin_voice_log_resolves_once_for_all_connected_admins() {
    let f = setup(0).await;
    let mut receivers = connect_admins(&f.state.hub, 3);

    f.log.take();
    routes::test_support::record_voice_event(
        &f.state,
        f.room.id,
        &f.author.id,
        "author",
        "join",
        None,
    )
    .await;
    let statements = f.log.take();
    let list_queries = statements
        .iter()
        .filter(|s| s.contains("FROM voice_events ORDER BY"))
        .count();
    assert_eq!(
        list_queries, EXPECTED_ADMIN_LIST_QUERIES,
        "the voice-log list must resolve in one query regardless of admin count: {statements:#?}"
    );

    let rendered: Vec<Arc<str>> = receivers.iter_mut().map(drain_voice_log).collect();
    assert_eq!(rendered.len(), 3);
    assert!(
        rendered.windows(2).all(|w| w[0] == w[1]),
        "every admin connection must receive the identical pre-rendered fragment"
    );
}

#[tokio::test]
async fn admin_reports_changed_resolves_once_for_all_connected_admins() {
    let f = setup(0).await;
    let mut receivers = connect_admins(&f.state.hub, 4);

    f.log.take();
    routes::test_support::broadcast_reports_changed(&f.state).await;
    let statements = f.log.take();
    let list_queries = statements
        .iter()
        .filter(|s| s.contains("FROM message_reports"))
        .count();
    assert_eq!(
        list_queries, EXPECTED_ADMIN_LIST_QUERIES,
        "the open-report list must resolve in one query regardless of admin count: {statements:#?}"
    );

    let rendered: Vec<Arc<str>> = receivers
        .iter_mut()
        .map(
            |rx| match rx.try_recv().expect("admin connection received an event") {
                ChatEvent::AdminReportChanged { rendered } => rendered,
                other => panic!("expected AdminReportChanged, got {other:?}"),
            },
        )
        .collect();
    assert_eq!(rendered.len(), 4);
    assert!(
        rendered.windows(2).all(|w| w[0] == w[1]),
        "every admin connection must receive the identical pre-rendered fragment"
    );
}

#[tokio::test]
async fn admin_support_changed_resolves_once_for_all_connected_admins() {
    let f = setup(0).await;
    let mut receivers = connect_admins(&f.state.hub, 4);

    f.log.take();
    routes::test_support::broadcast_support_changed(&f.state).await;
    let statements = f.log.take();
    let list_queries = statements
        .iter()
        .filter(|s| s.contains("FROM support_tickets"))
        .count();
    assert_eq!(
        list_queries, EXPECTED_ADMIN_LIST_QUERIES,
        "the open-ticket list must resolve in one query regardless of admin count: {statements:#?}"
    );

    let rendered: Vec<Arc<str>> = receivers
        .iter_mut()
        .map(
            |rx| match rx.try_recv().expect("admin connection received an event") {
                ChatEvent::AdminSupportChanged { rendered } => rendered,
                other => panic!("expected AdminSupportChanged, got {other:?}"),
            },
        )
        .collect();
    assert_eq!(rendered.len(), 4);
    assert!(
        rendered.windows(2).all(|w| w[0] == w[1]),
        "every admin connection must receive the identical pre-rendered fragment"
    );
}
