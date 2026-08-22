//! LC-75: outgoing webhooks - matching/enqueue, signed delivery, retry +
//! backoff, and auto-disable. Operates on a chat pool directly plus a local
//! HTTP receiver for the delivery path.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::Router;
use lets_chat::db::outgoing_webhooks as owh;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

mod common;

/// Captured request at the local receiver.
#[derive(Clone, Default)]
struct Captured {
    body: String,
    signature: String,
    event: String,
    timestamp: String,
    count: usize,
}

type Shared = Arc<Mutex<Captured>>;

/// Spawn a local receiver returning `status`. Records the last request.
async fn spawn_receiver(status: u16) -> (String, Shared) {
    let shared: Shared = Arc::new(Mutex::new(Captured::default()));
    let app = Router::new()
        .route(
            "/hook",
            post(
                |State((sh, st)): State<(Shared, u16)>, headers: HeaderMap, body: String| async move {
                    let mut c = sh.lock().unwrap();
                    c.body = body;
                    c.signature = headers
                        .get("X-LetsChat-Signature")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    c.event = headers
                        .get("X-LetsChat-Event")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    c.timestamp = headers
                        .get("X-LetsChat-Timestamp")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    c.count += 1;
                    axum::http::StatusCode::from_u16(st).unwrap()
                },
            ),
        )
        .with_state((shared.clone(), status));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/hook"), shared)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}

fn hmac_hex(secret: &str, body: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[tokio::test]
async fn enqueue_matches_event_and_scope() {
    let chat = common::pool("chat").await;
    // Global webhook subscribed to message.posted only.
    owh::insert(
        &chat,
        "global",
        None,
        "message.posted",
        "http://x/",
        "s",
        "admin",
    )
    .await
    .unwrap();

    lets_chat::outgoing::enqueue(&chat, "message.posted", 1, serde_json::json!({"a":1})).await;
    lets_chat::outgoing::enqueue(&chat, "message.edited", 1, serde_json::json!({"a":2})).await;

    let due = owh::due_deliveries(&chat, 100).await.unwrap();
    assert_eq!(due.len(), 1, "only the subscribed event enqueues");
    assert_eq!(due[0].event, "message.posted");
    assert!(due[0].payload.contains("\"version\":\"1\""));
    assert!(due[0].payload.contains("\"event\":\"message.posted\""));
}

#[tokio::test]
async fn delivery_succeeds_and_is_signed() {
    let chat = common::pool("chat").await;
    let (url, recv) = spawn_receiver(200).await;
    let secret = "shh-secret";
    let id = owh::insert(
        &chat,
        "global",
        None,
        "message.posted",
        &url,
        secret,
        "admin",
    )
    .await
    .unwrap();
    lets_chat::outgoing::enqueue(&chat, "message.posted", 1, serde_json::json!({"x":1})).await;

    let stats = lets_chat::outgoing::run_delivery_tick_unchecked(&chat, &client())
        .await
        .unwrap();
    assert_eq!(stats.delivered, 1);

    let c = recv.lock().unwrap().clone();
    assert_eq!(c.count, 1);
    assert_eq!(c.event, "message.posted");
    // Signature is over `timestamp.body` (replay-resistant).
    let signed = format!("{}.{}", c.timestamp, c.body);
    let expected = format!("sha256={}", hmac_hex(secret, &signed));
    assert_eq!(c.signature, expected, "HMAC over timestamp.body");
    assert!(!c.timestamp.is_empty(), "timestamp header present");

    // Delivery + webhook state updated.
    let due = owh::due_deliveries(&chat, 100).await.unwrap();
    assert!(due.is_empty(), "no pending deliveries after success");
    let wh = owh::get(&chat, id).await.unwrap().unwrap();
    assert!(wh.last_success_at.is_some());
    assert_eq!(wh.consecutive_failures, 0);
}

#[tokio::test]
async fn failed_delivery_reschedules_with_backoff() {
    let chat = common::pool("chat").await;
    let (url, _recv) = spawn_receiver(500).await;
    owh::insert(&chat, "global", None, "message.posted", &url, "s", "admin")
        .await
        .unwrap();
    lets_chat::outgoing::enqueue(&chat, "message.posted", 1, serde_json::json!({})).await;

    let stats = lets_chat::outgoing::run_delivery_tick_unchecked(&chat, &client())
        .await
        .unwrap();
    assert_eq!(stats.retried, 1, "5xx schedules a retry");
    assert_eq!(stats.delivered, 0);

    // Still pending, attempt bumped, scheduled in the future (not due now).
    let due_now = owh::due_deliveries(&chat, 100).await.unwrap();
    assert!(due_now.is_empty(), "rescheduled past now");
    let any: (i64, Option<String>) =
        sqlx::query_as("SELECT attempt, delivered_at FROM outgoing_webhook_deliveries LIMIT 1")
            .fetch_one(&chat)
            .await
            .unwrap();
    assert_eq!(any.0, 1, "attempt incremented");
    assert!(any.1.is_none(), "not delivered");
}

#[tokio::test]
async fn delivery_blocks_non_public_url_and_is_terminal() {
    // LC-152: a webhook URL whose host resolves to a non-public address must be
    // rejected at delivery time (before connecting) and not retried, even
    // though it passed the creation-time string check.
    let chat = common::pool("chat").await;
    owh::insert(
        &chat,
        "global",
        None,
        "message.posted",
        "http://127.0.0.1:9/hook",
        "s",
        "admin",
    )
    .await
    .unwrap();
    lets_chat::outgoing::enqueue(&chat, "message.posted", 1, serde_json::json!({})).await;

    // LC-152: production `run_delivery_tick` (no client param) goes through
    // `http_client::outbound_post`, which refuses loopback. This test
    // exercises the production path on purpose - the assertion is that
    // the loopback delivery is marked failed by the helper's filter.
    let stats = lets_chat::outgoing::run_delivery_tick(&chat).await.unwrap();
    assert_eq!(stats.failed, 1, "loopback URL must be blocked");
    assert_eq!(stats.delivered, 0);
    // Terminal: marked failed, not rescheduled for a later tick.
    assert!(
        owh::due_deliveries(&chat, 100).await.unwrap().is_empty(),
        "blocked delivery must not be retried"
    );
}

#[tokio::test]
async fn record_failure_auto_disables_at_threshold() {
    let chat = common::pool("chat").await;
    let id = owh::insert(
        &chat,
        "global",
        None,
        "message.posted",
        "http://x/",
        "s",
        "admin",
    )
    .await
    .unwrap();
    owh::record_failure(&chat, id, 2).await.unwrap();
    assert!(owh::get(&chat, id)
        .await
        .unwrap()
        .unwrap()
        .disabled_at
        .is_none());
    owh::record_failure(&chat, id, 2).await.unwrap();
    let wh = owh::get(&chat, id).await.unwrap().unwrap();
    assert!(wh.disabled_at.is_some(), "auto-disabled at threshold");

    // Re-enable clears the failure counter.
    owh::set_enabled(&chat, id, true).await.unwrap();
    let wh = owh::get(&chat, id).await.unwrap().unwrap();
    assert!(wh.disabled_at.is_none());
    assert_eq!(wh.consecutive_failures, 0);
}

#[tokio::test]
async fn disabled_webhook_does_not_match() {
    let chat = common::pool("chat").await;
    let id = owh::insert(
        &chat,
        "global",
        None,
        "message.posted",
        "http://x/",
        "s",
        "admin",
    )
    .await
    .unwrap();
    owh::set_enabled(&chat, id, false).await.unwrap();
    lets_chat::outgoing::enqueue(&chat, "message.posted", 1, serde_json::json!({})).await;
    assert!(
        owh::due_deliveries(&chat, 100).await.unwrap().is_empty(),
        "disabled webhook is not matched"
    );
}

#[tokio::test]
async fn scope_matching_room_enclave_global() {
    let chat = common::pool("chat").await;
    let g = owh::insert(
        &chat,
        "global",
        None,
        "message.posted",
        "http://x/",
        "s",
        "a",
    )
    .await
    .unwrap();
    let e = owh::insert(
        &chat,
        "enclave",
        Some(5),
        "message.posted",
        "http://x/",
        "s",
        "a",
    )
    .await
    .unwrap();
    let r = owh::insert(
        &chat,
        "room",
        Some(7),
        "message.posted",
        "http://x/",
        "s",
        "a",
    )
    .await
    .unwrap();

    // Room 7 in enclave 5: all three match.
    let mut m = owh::matching(&chat, "message.posted", 7, Some(5))
        .await
        .unwrap();
    m.sort();
    let mut want = vec![g, e, r];
    want.sort();
    assert_eq!(m, want);

    // Room 7, no enclave: global + room only (enclave webhook excluded).
    let m = owh::matching(&chat, "message.posted", 7, None)
        .await
        .unwrap();
    assert!(m.contains(&g) && m.contains(&r) && !m.contains(&e));

    // Different room in enclave 5: global + enclave, not room 7.
    let m = owh::matching(&chat, "message.posted", 99, Some(5))
        .await
        .unwrap();
    assert!(m.contains(&g) && m.contains(&e) && !m.contains(&r));
}

#[tokio::test]
async fn rotate_secret_changes_it() {
    let chat = common::pool("chat").await;
    let id = owh::insert(
        &chat,
        "global",
        None,
        "message.posted",
        "http://x/",
        "old",
        "admin",
    )
    .await
    .unwrap();
    owh::rotate_secret(&chat, id, "new").await.unwrap();
    assert_eq!(
        owh::get(&chat, id).await.unwrap().unwrap().signing_secret,
        "new"
    );
}
