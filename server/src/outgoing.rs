//! LC-75: outgoing webhooks. Event producers call [`enqueue`] to persist a
//! delivery row per matching subscription; [`run_delivery_tick`] (driven by a
//! background loop in `main`) POSTs the signed payload with retries +
//! exponential backoff and auto-disables a webhook after repeated failures.
//!
//! Payload schema is stable + versioned:
//! `{"version":"1","event":"message.posted","room_id":N,"data":{...}}`.
//! Each POST carries `X-LetsChat-Event`, `X-LetsChat-Timestamp`, and
//! `X-LetsChat-Signature: sha256=<hmac>` over the raw body, keyed by the
//! webhook's `signing_secret`. URLs and secrets are never logged.

use sqlx::{Row, SqlitePool};

use crate::db;

/// Schema version embedded in every payload.
pub const PAYLOAD_VERSION: &str = "1";

/// Backoff (seconds) indexed by the just-failed attempt number. Six entries =
/// six attempts before a delivery is marked permanently failed.
const BACKOFF_SECS: [i64; 6] = [1, 4, 16, 60, 300, 1800];
const MAX_ATTEMPTS: i64 = 6;
/// Consecutive failed deliveries before a webhook auto-disables.
const DISABLE_THRESHOLD: i64 = 5;
/// Delivery rows retained per webhook (older rows pruned each tick).
const RETENTION_PER_WEBHOOK: i64 = 50;
/// Max deliveries processed per tick.
const BATCH: i64 = 100;
/// Stored slice of a failing response body, for the admin history.
const RESPONSE_SNIPPET: usize = 1000;

/// HMAC-SHA256 hex of `body` keyed by the webhook's signing secret.
fn sign(secret: &str, body: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC any key length");
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Enqueue a delivery for every enabled webhook subscribed to `event` whose
/// scope matches `room_id`. Best-effort: errors are logged, never propagated
/// to the request path.
pub async fn enqueue(chat: &SqlitePool, event: &str, room_id: i64, data: serde_json::Value) {
    let enclave_id: Option<i64> = match sqlx::query("SELECT enclave_id FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(chat)
        .await
    {
        Ok(Some(r)) => r.get::<Option<i64>, _>("enclave_id"),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "outgoing webhook: enclave lookup failed");
            return;
        }
    };
    let ids = match db::outgoing_webhooks::matching(chat, event, room_id, enclave_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "outgoing webhook: match query failed");
            return;
        }
    };
    if ids.is_empty() {
        return;
    }
    let payload = serde_json::json!({
        "version": PAYLOAD_VERSION,
        "event": event,
        "room_id": room_id,
        "data": data,
    });
    let body = serde_json::to_string(&payload).unwrap_or_default();
    for id in ids {
        if let Err(e) = db::outgoing_webhooks::enqueue_delivery(chat, id, event, &body).await {
            tracing::warn!(error = %e, webhook_id = id, "outgoing webhook: enqueue failed");
        }
    }
}

#[derive(Default, Debug)]
pub struct DeliveryStats {
    pub delivered: usize,
    pub retried: usize,
    pub failed: usize,
}

/// Attempt all due deliveries once. Driven by the background loop. Enforces the
/// LC-152 delivery-time SSRF guard on every target URL.
pub async fn run_delivery_tick(
    chat: &SqlitePool,
    client: &reqwest::Client,
) -> sqlx::Result<DeliveryStats> {
    deliver_due(chat, client, true).await
}

/// Test seam: a delivery tick with the SSRF guard disabled, so the delivery-path
/// tests can target a loopback receiver (which the guard would otherwise, and in
/// production correctly does, reject). Production always uses `run_delivery_tick`.
#[doc(hidden)]
pub async fn run_delivery_tick_unchecked(
    chat: &SqlitePool,
    client: &reqwest::Client,
) -> sqlx::Result<DeliveryStats> {
    deliver_due(chat, client, false).await
}

async fn deliver_due(
    chat: &SqlitePool,
    client: &reqwest::Client,
    check_ssrf: bool,
) -> sqlx::Result<DeliveryStats> {
    let due = db::outgoing_webhooks::due_deliveries(chat, BATCH).await?;
    let mut stats = DeliveryStats::default();
    for d in due {
        let Some(t) = db::outgoing_webhooks::target(chat, d.webhook_id).await? else {
            // Webhook deleted out from under the delivery: drop it.
            db::outgoing_webhooks::mark_failed(chat, d.id, 0, Some("webhook removed")).await?;
            continue;
        };
        if t.disabled_at.is_some() {
            db::outgoing_webhooks::mark_failed(chat, d.id, 0, Some("webhook disabled")).await?;
            continue;
        }

        // LC-152: re-validate the destination at delivery time. Creation only
        // does a string check that lets any hostname through, so a URL whose
        // host resolves to an internal / cloud-metadata address would be
        // fetched here with signed payloads. Resolve and reject non-public
        // targets (and non-http(s) schemes) before connecting. Terminal, not
        // retried: a blocked URL will not become public on a later tick.
        let url_ok = !check_ssrf
            || match url::Url::parse(&t.url) {
                Ok(u) => {
                    matches!(u.scheme(), "http" | "https")
                        && crate::ssrf::host_resolves_public(&u).await
                }
                Err(_) => false,
            };
        if !url_ok {
            db::outgoing_webhooks::mark_failed(chat, d.id, 0, Some("blocked: non-public URL"))
                .await?;
            stats.failed += 1;
            continue;
        }

        // Sign `timestamp.body` (not the body alone) so a captured delivery
        // cannot be replayed indefinitely: the signature is bound to the
        // timestamp the receiver also checks.
        let timestamp = chrono::Utc::now().timestamp();
        let signature = sign(&t.signing_secret, &format!("{timestamp}.{}", d.payload));
        let resp = client
            .post(&t.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("X-LetsChat-Event", &d.event)
            .header("X-LetsChat-Timestamp", timestamp.to_string())
            .header("X-LetsChat-Signature", format!("sha256={signature}"))
            .body(d.payload.clone())
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let status = r.status().as_u16() as i64;
                db::outgoing_webhooks::mark_delivered(chat, d.id, status).await?;
                db::outgoing_webhooks::record_success(chat, d.webhook_id).await?;
                stats.delivered += 1;
            }
            Ok(r) => {
                let status = r.status().as_u16() as i64;
                let snippet = r
                    .text()
                    .await
                    .ok()
                    .map(|b| b.chars().take(RESPONSE_SNIPPET).collect::<String>());
                handle_failure(chat, &d, status, snippet.as_deref(), &mut stats).await?;
            }
            Err(_) => {
                // Network/timeout error: status 0 (the URL is intentionally
                // not included in the log to avoid leaking it).
                handle_failure(chat, &d, 0, None, &mut stats).await?;
            }
        }
    }
    db::outgoing_webhooks::prune_deliveries(chat, RETENTION_PER_WEBHOOK)
        .await
        .ok();
    Ok(stats)
}

async fn handle_failure(
    chat: &SqlitePool,
    d: &db::outgoing_webhooks::DueDelivery,
    status: i64,
    body: Option<&str>,
    stats: &mut DeliveryStats,
) -> sqlx::Result<()> {
    let next_attempt = d.attempt + 1;
    if next_attempt < MAX_ATTEMPTS {
        let delay = BACKOFF_SECS[d.attempt.clamp(0, BACKOFF_SECS.len() as i64 - 1) as usize];
        db::outgoing_webhooks::reschedule(chat, d.id, status, body, delay).await?;
        stats.retried += 1;
    } else {
        db::outgoing_webhooks::mark_failed(chat, d.id, status, body).await?;
        db::outgoing_webhooks::record_failure(chat, d.webhook_id, DISABLE_THRESHOLD).await?;
        stats.failed += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_stable_hmac() {
        // Known-answer: HMAC-SHA256("secret", "body") hex.
        let got = sign("secret", "body");
        assert_eq!(got.len(), 64);
        // Deterministic.
        assert_eq!(got, sign("secret", "body"));
        assert_ne!(got, sign("other", "body"));
    }
}
