//! LC-780: a broadcast message resolves its viewer-independent render inputs
//! once, in the producer, instead of once per connected recipient.
//!
//! Each recipient's WebSocket task renders the event for its own viewer. The
//! author, attachments, mentions, grouping predecessor, quote preview, room emoji
//! set and the poll / follow-up existence checks are identical for every viewer,
//! so they live on the event's context (`NewMessageContext`,
//! `EditedMessageContext`, `ThreadReplyContext`). These tests count the SQL
//! statements the per-recipient render issues (the `sqlx::query` tracing events
//! `RUST_LOG=sqlx=debug` shows) and pin two contracts: a render never repeats a
//! statement its producer already ran, and a recipient's cost does not depend on
//! how many other recipients exist.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use lets_chat::models::{Message, User};
use lets_chat::routes::render_context::{
    EditedMessageContext, NewMessageContext, ThreadReplyContext,
};
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
    moderator: User,
    members: Vec<User>,
    message: Message,
}

/// The General enclave room (id 1) holding an author, a site admin (a room
/// moderator) and `member_count` ordinary members, with one message posted by
/// the author.
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
    let moderator = load("moderator").await;
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
        moderator,
        members,
        message,
    }
}

/// Render the message for `viewer` with the room open in the foreground and
/// return the HTML plus the statements the render issued.
async fn deliver(
    f: &Fixture,
    ctx: &NewMessageContext,
    viewer: &User,
) -> (Option<String>, Vec<String>) {
    let subscribed = Arc::new(Mutex::new(HashSet::from([f.room.id])));
    f.log.take();
    let html =
        routes::render_new_message_or_bump(&f.state, &f.message, ctx, None, viewer, &subscribed)
            .await;
    (html, f.log.take())
}

#[tokio::test]
async fn recipient_render_repeats_no_statement_the_producer_ran() {
    let f = setup(3).await;
    f.log.take();
    let ctx = NewMessageContext::load(&f.state, &f.message, &f.room)
        .await
        .unwrap();
    let producer: HashSet<String> = f.log.take().into_iter().collect();
    assert!(
        producer.len() >= 6,
        "the producer must resolve the shared inputs itself, ran: {producer:#?}"
    );

    for viewer in [&f.author, &f.moderator, &f.members[0]] {
        let (html, statements) = deliver(&f, &ctx, viewer).await;
        assert!(html.is_some(), "{} renders the message", viewer.username);
        let repeated: Vec<&String> = statements
            .iter()
            .filter(|s| producer.contains(*s))
            .collect();
        assert!(
            repeated.is_empty(),
            "{}'s render repeated producer statements: {repeated:#?}",
            viewer.username
        );
    }
}

#[tokio::test]
async fn recipient_render_cost_is_independent_of_recipient_count() {
    let f = setup(8).await;
    let ctx = NewMessageContext::load(&f.state, &f.message, &f.room)
        .await
        .unwrap();

    let mut per_recipient = Vec::new();
    for member in &f.members {
        let (html, statements) = deliver(&f, &ctx, member).await;
        assert!(html.is_some());
        per_recipient.push(statements.len());
    }
    // Every ordinary member costs the same, so N recipients cost N times a
    // constant: the delta from 1 recipient to 8 is 7 times that constant.
    assert!(
        per_recipient.windows(2).all(|w| w[0] == w[1]),
        "per-recipient statement counts differ: {per_recipient:?}"
    );
}

/// The author's own connection: the block check plus the render, with no
/// moderator lookup and no read-watermark write. Before LC-780 this ran 17
/// statements; what remains is only per-viewer work: the block check, the
/// viewer's personal emoji, the `#channel` links their access allows, and the
/// two statements behind the LC-489 "Seen by" bar.
#[tokio::test]
async fn author_echo_runs_only_per_viewer_statements() {
    let f = setup(1).await;
    let ctx = NewMessageContext::load(&f.state, &f.message, &f.room)
        .await
        .unwrap();
    let subscribed = Arc::new(Mutex::new(HashSet::new()));
    f.log.take();
    let html = routes::render_new_message_or_bump(
        &f.state,
        &f.message,
        &ctx,
        None,
        &f.author,
        &subscribed,
    )
    .await;
    let statements = f.log.take();
    assert!(html.is_some());
    assert!(
        statements.len() <= 5,
        "author echo ran {} statements: {statements:#?}",
        statements.len()
    );
}

#[tokio::test]
async fn edited_message_render_repeats_no_statement_the_producer_ran() {
    let f = setup(3).await;
    f.log.take();
    let ctx = EditedMessageContext::load(&f.state, f.message.id)
        .await
        .unwrap()
        .expect("the message exists");
    let producer: HashSet<String> = f.log.take().into_iter().collect();
    assert!(
        producer.len() >= 8,
        "the producer must resolve the shared inputs itself, ran: {producer:#?}"
    );

    for viewer in [&f.author, &f.moderator, &f.members[0]] {
        f.log.take();
        let html = routes::test_support::render_edited_message(&f.state, &ctx, viewer).await;
        let statements = f.log.take();
        assert!(html.is_some(), "{} renders the edit", viewer.username);
        let repeated: Vec<&String> = statements
            .iter()
            .filter(|s| producer.contains(*s))
            .collect();
        assert!(
            repeated.is_empty(),
            "{}'s edit render repeated producer statements: {repeated:#?}",
            viewer.username
        );
    }
}

/// LC-783: a reaction toggle broadcasts the same `EditedMessageContext`, so a
/// recipient's reaction-bar render must not repeat any statement the producer
/// already ran, same contract as an edit.
#[tokio::test]
async fn reaction_render_repeats_no_statement_the_producer_ran() {
    let f = setup(3).await;
    db::chat::toggle_reaction(&f.state.chat, f.message.id, &f.members[0].id, "\u{1f44d}")
        .await
        .unwrap();
    f.log.take();
    let ctx = EditedMessageContext::load(&f.state, f.message.id)
        .await
        .unwrap()
        .expect("the message exists");
    let producer: HashSet<String> = f.log.take().into_iter().collect();
    assert!(
        producer.len() >= 8,
        "the producer must resolve the shared inputs itself, ran: {producer:#?}"
    );

    for viewer in [&f.author, &f.moderator, &f.members[0]] {
        f.log.take();
        let html = routes::test_support::render_reaction_bar(&f.state, &ctx, &viewer.id).await;
        let statements = f.log.take();
        assert!(
            html.is_some(),
            "{} renders the reaction bar",
            viewer.username
        );
        let repeated: Vec<&String> = statements
            .iter()
            .filter(|s| producer.contains(*s))
            .collect();
        assert!(
            repeated.is_empty(),
            "{}'s reaction-bar render repeated producer statements: {repeated:#?}",
            viewer.username
        );
    }
}

/// LC-783: the ack-required toggle shares the same roster the producer already
/// resolved, so `acked_by_me` is a membership check, not a fresh query.
#[tokio::test]
async fn ack_required_render_repeats_no_statement_the_producer_ran() {
    let f = setup(3).await;
    db::acks::set_required(&f.state.chat, f.message.id, f.room.id, &f.author.id)
        .await
        .unwrap();
    db::acks::acknowledge(&f.state.chat, f.message.id, &f.members[0].id)
        .await
        .unwrap();
    f.log.take();
    let ctx = EditedMessageContext::load(&f.state, f.message.id)
        .await
        .unwrap()
        .expect("the message exists");
    let producer: HashSet<String> = f.log.take().into_iter().collect();
    assert!(
        producer.len() >= 8,
        "the producer must resolve the shared inputs itself, ran: {producer:#?}"
    );

    for viewer in [&f.author, &f.moderator, &f.members[0]] {
        f.log.take();
        let html = routes::test_support::render_ack_required(&f.state, viewer, &ctx).await;
        let statements = f.log.take();
        assert!(html.is_some(), "{} renders the ack bar", viewer.username);
        let repeated: Vec<&String> = statements
            .iter()
            .filter(|s| producer.contains(*s))
            .collect();
        assert!(
            repeated.is_empty(),
            "{}'s ack-required render repeated producer statements: {repeated:#?}",
            viewer.username
        );
    }
}

/// LC-783: pinning/unpinning shares the same producer-resolved context as an
/// edit; a recipient's pin-event render must not repeat its statements.
#[tokio::test]
async fn pin_event_render_repeats_no_statement_the_producer_ran() {
    let f = setup(3).await;
    db::pinned::pin_message(&f.state.chat, f.message.id, f.room.id, &f.author.id)
        .await
        .unwrap();
    f.log.take();
    let ctx = EditedMessageContext::load(&f.state, f.message.id)
        .await
        .unwrap()
        .expect("the message exists");
    let producer: HashSet<String> = f.log.take().into_iter().collect();
    assert!(
        producer.len() >= 8,
        "the producer must resolve the shared inputs itself, ran: {producer:#?}"
    );

    for viewer in [&f.author, &f.moderator, &f.members[0]] {
        f.log.take();
        let html = routes::test_support::render_pin_event(&f.state, viewer, &ctx).await;
        let statements = f.log.take();
        assert!(html.is_some(), "{} renders the pin event", viewer.username);
        let repeated: Vec<&String> = statements
            .iter()
            .filter(|s| producer.contains(*s))
            .collect();
        assert!(
            repeated.is_empty(),
            "{}'s pin-event render repeated producer statements: {repeated:#?}",
            viewer.username
        );
    }
}

#[tokio::test]
async fn thread_reply_render_repeats_no_statement_the_producer_ran() {
    let f = setup(3).await;
    let reply_id = db::chat::insert_reply(
        &f.state.chat,
        f.room.id,
        &f.members[0].id,
        "r",
        f.message.id,
    )
    .await
    .unwrap();
    let raw = db::chat::get_message(&f.state.chat, reply_id)
        .await
        .unwrap()
        .unwrap();
    let reply = Message {
        id: raw.id,
        room_id: raw.room_id,
        user_id: raw.user_id,
        author_name: f.members[0].username.clone(),
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
    f.log.take();
    let ctx = ThreadReplyContext::load(&f.state, f.message.id, &reply)
        .await
        .unwrap();
    let producer: HashSet<String> = f.log.take().into_iter().collect();
    assert!(
        producer.len() >= 6,
        "the producer must resolve the shared inputs itself, ran: {producer:#?}"
    );

    for viewer in [&f.author, &f.moderator, &f.members[1]] {
        f.log.take();
        let html =
            routes::test_support::render_thread_reply(&f.state, f.message.id, &reply, &ctx, viewer)
                .await;
        let statements = f.log.take();
        assert!(html.is_some(), "{} renders the reply", viewer.username);
        let repeated: Vec<&String> = statements
            .iter()
            .filter(|s| producer.contains(*s))
            .collect();
        assert!(
            repeated.is_empty(),
            "{}'s thread-reply render repeated producer statements: {repeated:#?}",
            viewer.username
        );
    }
}
