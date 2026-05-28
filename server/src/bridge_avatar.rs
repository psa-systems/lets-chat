//! LC-78-AVATAR-PROXY: foreign bridge-avatar fetch + cache.
//!
//! Called fire-and-forget from the bridge-messages POST handler when a new
//! `bridge_avatar_proxies` row lands in `pending` status. Resolves the
//! foreign URL, fetches up to 1 MiB, sniffs the content type via magic
//! bytes, re-encodes through the upload pipeline (EXIF / XMP / IPTC strip),
//! writes the bytes atomically to `bridge_avatars_dir()`, and flips the
//! row to `ok` / `failed`. Never panics: the decoder spike
//! (`spike_image_decoder_hostile_corpus`) covers `image 0.25` against the
//! hostile corpus, and `spawn_blocking` gives a second layer of panic
//! isolation. Never blocks the POST: errors land in the DB row's
//! `failure_reason` for the admin to read, and the render falls back to
//! initials via the `<img onerror>` path.
//!
//! Production callers must pre-validate the URL via
//! `crate::ssrf::host_resolves_public` to keep DNS-rebinding small (the
//! re-resolve here happens at fetch time, but there is a TOCTOU window
//! between this call and `reqwest`'s own DNS resolution shared with the
//! LC-152 outgoing-webhook delivery path).

use crate::db::bridge_avatar_proxies;
use sqlx::SqlitePool;
use std::path::Path;
use std::time::Duration;
use url::Url;

/// Hard cap on bytes fetched from the foreign server. Per-fetch, the
/// decoder spike confirms `image 0.25` handles up to this size without
/// panicking on hostile inputs. A foreign avatar over this size is
/// rejected; the daemon should resize before submitting.
pub const MAX_BYTES: usize = 1 << 20; // 1 MiB

/// Hard cap on the foreign GET. Includes connect + headers + body read.
/// Tighter than the LC-152 outgoing-webhook 10s because a slow homeserver
/// here just means initials-forever for that one avatar; the daemon's
/// next message still flows.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Allowlist of magic-byte-sniffed MIME types. Same set the uploads
/// pipeline accepts; `pipeline::process_image` would also reject anything
/// else, but checking first avoids spinning up the decoder for clearly
/// hostile input.
const ALLOWED_MIME: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

pub async fn fetch_and_cache(chat: &SqlitePool, hash: &str, foreign_url: &str) {
    // LC-152: all URL validation (parse + scheme + host_resolves_public)
    // happens inside `http_client::outbound_get`. Literal-IP URLs are
    // caught here (the resolver never sees them); hostname URLs are also
    // caught here AND again by the resolver inside reqwest's resolution
    // path, two layers covering two distinct attack classes.
    let req = match crate::http_client::outbound_get(foreign_url).await {
        Ok(r) => r,
        Err(crate::http_client::OutboundError::HostNotPublic) => {
            mark(chat, hash, "blocked: non-public foreign host").await;
            return;
        }
        Err(crate::http_client::OutboundError::UnsupportedScheme(s)) => {
            mark(chat, hash, &format!("non-http(s) scheme: {s}")).await;
            return;
        }
        Err(crate::http_client::OutboundError::InvalidUrl(_)) => {
            mark(chat, hash, "invalid foreign url").await;
            return;
        }
    };
    fetch_and_cache_inner(chat, req, hash).await;
}

/// Test seam: skip the SSRF guard so a test can target a loopback
/// receiver. Production callers go through `fetch_and_cache`. Mirrors the
/// LC-75 `run_delivery_tick_unchecked` shape.
#[doc(hidden)]
pub async fn fetch_and_cache_unchecked(
    chat: &SqlitePool,
    client: &reqwest::Client,
    hash: &str,
    foreign_url: &str,
) {
    let req = client.get(foreign_url);
    fetch_and_cache_inner(chat, req, hash).await;
}

async fn fetch_and_cache_inner(chat: &SqlitePool, req: reqwest::RequestBuilder, hash: &str) {
    let bytes = match req.timeout(FETCH_TIMEOUT).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            mark(chat, hash, &format!("http {}", r.status().as_u16())).await;
            return;
        }
        Err(e) => {
            mark(chat, hash, &format!("fetch: {e}")).await;
            return;
        }
    };

    // Streamed read with a hard cap. `Content-Length` is advisory only:
    // a hostile server could lie and stream more bytes, so the cap is
    // enforced on every chunk.
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut stream = bytes;
    loop {
        let chunk = match stream.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                mark(chat, hash, &format!("read: {e}")).await;
                return;
            }
        };
        if buf.len() + chunk.len() > MAX_BYTES {
            mark(chat, hash, "exceeds 1MiB cap").await;
            return;
        }
        buf.extend_from_slice(&chunk);
    }

    // Write to a per-hash temp file in the cache dir so the atomic rename
    // at the end is intra-filesystem.
    let final_path = crate::db::bridge_avatars_dir().join(hash);
    let tmp_path = crate::db::bridge_avatars_dir().join(format!(".tmp-{hash}"));
    if let Err(e) = tokio::fs::write(&tmp_path, &buf).await {
        mark(chat, hash, &format!("temp write: {e}")).await;
        return;
    }

    // Magic-byte sniff. The Content-Type header is foreign-controlled and
    // ignored; the sniffer reads file bytes only.
    let sniffed = match infer::get_from_path(&tmp_path).ok().flatten() {
        Some(t) => t.mime_type().to_string(),
        None => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            mark(chat, hash, "magic-byte sniff returned no type").await;
            return;
        }
    };
    if !ALLOWED_MIME.contains(&sniffed.as_str()) {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        mark(chat, hash, &format!("disallowed mime: {sniffed}")).await;
        return;
    }

    // Re-encode through the uploads pipeline. spawn_blocking isolates the
    // sync decoder + any panic (belt-and-suspenders on top of the spike
    // confirming image 0.25 does not panic on hostile inputs at 1MiB).
    let tmp_for_decode = tmp_path.clone();
    let mime_for_decode = sniffed.clone();
    let decoded = tokio::task::spawn_blocking(move || {
        crate::uploads::pipeline::process_image(&tmp_for_decode, &mime_for_decode)
    })
    .await;
    let _ = tokio::fs::remove_file(&tmp_path).await;
    let processed = match decoded {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            mark(chat, hash, &format!("decode: {e}")).await;
            return;
        }
        Err(join_err) => {
            // Panic from spawn_blocking. The spike says this should not
            // happen on the hostile corpus at the 1MiB cap, but the catch
            // is here as defense in depth (image-crate bump might regress).
            mark(chat, hash, &format!("decoder panic: {join_err}")).await;
            return;
        }
    };

    // Atomic rename onto the final path so a viewer that hits the proxy
    // endpoint mid-write sees either the old file (if a prior fetch wrote
    // one) or no file (404 -> initials fallback), never a partial.
    if let Err(e) = write_atomic(&final_path, &processed.original_bytes).await {
        mark(chat, hash, &format!("final write: {e}")).await;
        return;
    }

    let byte_size = processed.original_bytes.len() as i64;
    let _ = bridge_avatar_proxies::mark_ok(chat, hash, &processed.mime, byte_size).await;
}

async fn write_atomic(final_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = final_path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent dir"))?;
    let staging = parent.join(format!(
        ".rename-{}",
        final_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("x")
    ));
    tokio::fs::write(&staging, bytes).await?;
    tokio::fs::rename(&staging, final_path).await
}

async fn mark(chat: &SqlitePool, hash: &str, reason: &str) {
    tracing::warn!(target: "bridge_avatar", %hash, %reason, "bridge avatar fetch failed");
    let _ = bridge_avatar_proxies::mark_failed(chat, hash, reason).await;
}

/// LC-78-AVATAR-PROXY operator gate. Default ON in v2: the foreign-avatar
/// proxy is enabled and the bridge endpoint accepts `foreign_avatar` URLs.
/// Set `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false` (or `0`) to restore
/// the v1 reject-non-null behavior; the POST endpoint then 400s any
/// non-null `foreign_avatar` exactly as it did in v1, and the proxy GET
/// endpoint returns 404 for every hash. Read per-request so flipping the
/// flag does not require a restart, but the env::var lookup is microseconds
/// and not on a hot loop.
pub fn proxy_enabled() -> bool {
    match std::env::var("LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v == "false" || v == "0" || v == "no" || v == "off")
        }
        Err(_) => true,
    }
}

/// LC-78-AVATAR-PROXY GC sweep. Piggybacks on the hourly orphan tick.
/// Two passes:
///
/// 1. **Unreferenced retention.** Rows whose `last_seen_at` is older than
///    `older_than_days` AND that no live `messages.bridge_foreign_avatar`
///    row references are deleted, along with their on-disk file. `last_seen_at`
///    is bumped on every bridge-message POST that mentions the hash, so an
///    actively-bridged avatar stays cached forever.
///
/// 2. **Pending orphans.** Rows stuck in `pending` longer than
///    `pending_older_than_secs` are marked `failed` ("orphaned pending fetch").
///    The fetch task should complete within seconds; a row sitting in pending
///    for minutes means the task crashed or the process restarted before it
///    finished. Marking failed lets the render fall back to initials via
///    `<img onerror>` rather than rendering a permanently-pending state.
///
/// Returns counts for logging. Errors are returned so the caller can log,
/// not propagated up so a partial sweep is still useful.
pub async fn sweep_tick(
    chat: &SqlitePool,
    older_than_days: i64,
    pending_older_than_secs: i64,
) -> SweepStats {
    let mut stats = SweepStats::default();
    match crate::db::bridge_avatar_proxies::sweep_unreferenced(chat, older_than_days).await {
        Ok(hashes) => {
            for hash in hashes {
                let path = crate::db::bridge_avatars_dir().join(&hash);
                match tokio::fs::remove_file(&path).await {
                    Ok(_) => stats.files_deleted += 1,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        tracing::warn!(error = %e, %hash, "bridge avatar file unlink failed");
                        stats.errors += 1;
                    }
                }
                match crate::db::bridge_avatar_proxies::delete_by_hash(chat, &hash).await {
                    Ok(true) => stats.rows_deleted += 1,
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, %hash, "bridge avatar row delete failed");
                        stats.errors += 1;
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "bridge avatar unreferenced sweep failed");
            stats.errors += 1;
        }
    }
    match crate::db::bridge_avatar_proxies::sweep_pending_orphans(chat, pending_older_than_secs)
        .await
    {
        Ok(n) => stats.pending_marked_failed = n,
        Err(e) => {
            tracing::warn!(error = %e, "bridge avatar pending-orphan sweep failed");
            stats.errors += 1;
        }
    }
    stats
}

#[derive(Debug, Default)]
pub struct SweepStats {
    pub rows_deleted: usize,
    pub files_deleted: usize,
    pub pending_marked_failed: u64,
    pub errors: usize,
}

/// Compute the cache key for a foreign avatar URL. Canonicalization:
/// lowercase scheme + host, strip default ports, strip fragment. Path
/// stays as-is (Matrix media URLs are case-sensitive).
pub fn canonical_hash(foreign_url: &str) -> Option<String> {
    let mut u = Url::parse(foreign_url).ok()?;
    // Lowercase scheme + host. url::Url does this on parse for scheme; do
    // host manually since url preserves authority case in some edge cases.
    if let Some(host) = u.host_str() {
        let lower = host.to_lowercase();
        u.set_host(Some(&lower)).ok()?;
    }
    u.set_fragment(None);
    // Strip default ports: url::Url's serialization already omits them when
    // they match the scheme's well-known default, so no extra work needed
    // here (`url` 2.x: http -> 80, https -> 443).
    let canon = u.as_str();
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(canon.as_bytes());
    Some(hex::encode(h.finalize()))
}
