# Phase 16 - Web Push Notifications

## Goal

Deliver mention and DM notifications to users even when the lets-chat tab is
closed, via the W3C Web Push protocol. Push is the OS-level extension of the
Phase 14 in-app notification surface: same triggers (`ChatEvent::Mentioned`
events, including the implicit DM `kind: "dm"` variant), same snippet, same
target path. The user opts in via a new "Enable push notifications" checkbox
in `/settings`; on the next qualifying event, the page lazily registers a
service worker, subscribes via `PushManager`, and POSTs the subscription to
the server. Mention fan-out then dispatches a JSON payload to each of the
user's stored Push endpoints, gated by the per-room mute mode introduced in
Phase 15.

Browser scope: Chrome / Firefox / Edge (desktop and Android). iOS Safari and
macOS Safari are explicitly out of scope this phase (Apple uses APNs with a
separate VAPID flow and only supports Push for installed PWAs on iOS 16.4+).

Out of scope (deferred to later phases):

- iOS / macOS Safari Push, PWA install flow, `manifest.json` beyond what
  Chrome needs.
- Per-device "unregister this device" UI - lazy 410 cleanup is the only
  mechanism this phase.
- VAPID key rotation UI - a code-comment seam only.
- VAPID env-var override - keys are auto-generated and persisted on first
  boot.
- Background sync, offline shell, fetch interception inside the service
  worker. The SW handles `push` and `notificationclick` only.
- Bounded-concurrency or worker-pool fan-out. Current shape is
  `tokio::spawn` per (user x subscription). At today's scale (mention
  targets in single digits, devices per user 1-3) this is fine. If
  `@here` / `@channel` / `@everyone` lands later, fan-out shape needs
  revisiting.
- Push for ambient `NewMessage` events. Only `Mentioned` (room or DM
  kind) triggers Push.
- Email digest of missed Pushes.

## Architecture

- **Stack** (current truth): Axum 0.8 + Askama + HTMX. WebSocket payloads
  are pre-rendered HTML fragments tagged with `hx-swap-oob`. Push payloads
  are a different transport: JSON over HTTPS to the third-party Push
  service (FCM / Mozilla autopush / Edge), encrypted under per-subscription
  keys. The two paths share the `Mentioned` event but never share render
  code.
- **Mode parity.** Push compiles and runs in both `standalone` and `saas`
  binaries. No `#[cfg(feature = ...)]` gating on any Push code path.
- **VAPID keypair lifecycle.**
  - At first boot, the server generates a fresh ECDSA P-256 keypair via the
    `p256` crate. The private key is PEM-encoded (PKCS#8), AES-256-GCM
    encrypted using the existing `crate::crypto::seal` helper under the
    process-wide `LETS_CHAT_SECRET_KEY`-derived key, and persisted in a
    new singleton row of `settings.db.vapid_keypair`. The public key is
    encoded as a 65-byte raw uncompressed P-256 point (`0x04` prefix + X +
    Y), base64url-encoded, stored plaintext in the same row.
  - On every subsequent boot, the row is loaded, decrypted, and held as
    `Arc<VapidKeypair>` on `AppState`.
  - **Push is gated on `LETS_CHAT_SECRET_KEY` being configured**, exactly
    parallel to 2FA. When the env var is unset, the VAPID keypair is never
    generated, the `vapid` field on `AppState` is `None`, and:
    - `GET /push/vapid-public-key` returns 404.
    - `POST /push/subscribe` returns 404.
    - `push::dispatch` short-circuits with no work.
    - The settings checkbox renders disabled with explanatory help text.
- **Crypto reuse.** `crate::crypto::seal` / `crate::crypto::open` are
  already a generic, sealed-AES-256-GCM helper taking `&[u8; 32]` key plus
  arbitrary plaintext. They are not coupled to TOTP or SMTP. No factoring
  is required: the new `db::vapid` module imports them directly. (See
  "Things to confirm" for the SMTP-encryption follow-up.)
- **Subscription storage.** `auth.db.push_subscriptions(id, user_id,
  endpoint UNIQUE, p256dh_key, auth_key, user_agent, created_at,
  last_seen_at)`. Rationale for the schema choices:
  - **Lives in `auth.db`**, not `chat.db`. Push subscriptions are a pure
    user-credential concern and naturally co-locate with the new
    `notify_push_enabled` column on `users`.
  - **`UNIQUE(endpoint)`** alone, not `(user_id, endpoint)`. A Push
    endpoint represents a `(browser, application server)` pair - the
    browser's `pushManager.subscribe()` returns the same endpoint+keys
    for two users sharing a browser, because the keys are derived from
    the VAPID public key, not from the user. If user B logs in on a
    browser already subscribed for user A,
    `INSERT ... ON CONFLICT(endpoint) DO UPDATE SET user_id = excluded.user_id`
    flips the subscription to B. Last-write-wins matches the user-visible
    model: this browser pings whoever logged in last.
  - **`last_seen_at`** is bumped on every successful Push send. Rows that
    stop updating are diagnostic candidates for stale subscriptions; we
    do not act on them this phase but the column is the only way an admin
    can spot dead rows during incident response.
  - **No `id` lookups.** All accesses are by `user_id` or `endpoint`. The
    auto-increment `id` is for ergonomic deletes only.
- **`notify_push_enabled` column.** Boolean on `users`. Default 0 (opt-in,
  matches the user's privacy expectation: Push goes through a third-party
  service even when the browser is closed, and we should not subscribe
  silently). The column is the predicate the dispatch helper checks
  before fan-out.
- **`PushClient` trait** (in `server/src/push/mod.rs`):

  ```rust
  #[async_trait::async_trait]
  pub trait PushClient: Send + Sync {
      async fn send(&self, sub: &PushSubscription, payload: Bytes) -> Result<(), PushError>;
  }
  ```

  Two implementations:
  1. `IsahcPushClient` - production. Wraps `web_push::IsahcWebPushClient`.
     Builds a `WebPushMessage` with the VAPID signature and the
     subscription's keys, sends, maps errors. Constructed once at startup
     from the loaded VAPID keypair and held on `AppState` as
     `Arc<dyn PushClient>`.
  2. `MockPushClient` - test-only. A `Mutex<Vec<RecordedSend>>` that
     records every `send()` call without making a network request.
     Tests inject this in place of `IsahcPushClient` and assert on the
     recorded calls.
  No `send_batch`, no `subscribe`, no `notification-dispatch` abstraction.
  The trait has exactly one method.
- **`push::dispatch(state, user_id, room_id, kind, event)`.** Single
  helper invoked from each `Mentioned`-broadcast site. Responsibilities:
  1. If `state.vapid` is `None`, return immediately (Push disabled
     globally).
  2. Look up the recipient's `notify_push_enabled` flag. If false, return.
  3. If `kind != "dm"`, look up the recipient's `room_mute_mode` for
     `room_id` via `db::notifications::room_mute_mode`. If `MuteMode::All`,
     return. (`MuteMode::ExceptMentions` falls through and Push fires;
     same as the WS path's `Mentioned` arm.)
  4. Load all of the recipient's rows from `push_subscriptions`. If empty,
     return.
  5. Build the JSON payload via `push::payload::build(event)` once.
  6. For each subscription, `tokio::spawn` a task that calls
     `state.push_client.send(sub, payload.clone())`, branches on
     `PushError::EndpointGone` to `db::push_subscriptions::delete_by_endpoint`,
     logs `tracing::warn!` on other errors, and `bump_last_seen` on
     success. Failures past the `warn!` are dropped on the floor; this is
     the explicit fire-and-forget tradeoff (see "Out of scope").
- **Fan-out call sites.** Three places fan out `ChatEvent::Mentioned` today
  (all in `server/src/routes/room.rs`):
  1. `post_message`, room-mention loop (line ~330).
  2. `post_message`, DM branch (line ~352).
  3. `patch_message`, mention reconcile loop, `added` arm (line ~563).
  Each of those sites gains a `push::dispatch(...)` call directly after the
  existing `state.hub.broadcast_to_user(...)` call. `MentionCleared` does
  not trigger Push.

  **DM bypass site marker.** The DM branch in `post_message` is where
  `kind == "dm"` Mentioned events originate. The dispatch helper bypasses
  the room-mute check for this kind. A code comment at the bypass site
  reads:

  ```rust
  // FUTURE: when the DM-mute phase lands, this bypass becomes
  // conditional on dm_mute_state(user, peer).
  ```

  so future-us doesn't have to rediscover the seam.

  **Endpoint conflict edge case.** If user A has a Push send in flight to
  endpoint E at the moment user B logs into the same browser and POSTs
  `/push/subscribe`, the row for E flips to B mid-flight. A's pending Push
  may still arrive at the browser, where it is rendered for whoever holds
  the cookie session at that instant - which is now B. This is documented
  but accepted: browser-keyed sessions inherently have this property
  (cookie-only auth + browser-keyed subscription endpoint), and the race
  window is on the order of milliseconds. No locking added.
- **Service worker** (`server/assets/sw.js`, ~60 lines, served as
  `GET /sw.js`):
  - `push` event: parses the JSON payload, calls
    `clients.matchAll({ type: 'window', visible: true })` to gather visible
    page clients, and **suppresses `showNotification()` if any visible
    client has `pathname === data.target_path`**. This is baked in, not
    optional - the source carries a comment explaining the rationale (no
    OS ping for the room you are actively reading). Otherwise the SW
    calls `self.registration.showNotification(title, options)` with the
    payload-supplied fields.
  - `notificationclick` event: closes the notification, calls
    `clients.matchAll({ type: 'window', includeUncontrolled: true })`,
    focuses an existing client whose `pathname === data.target_path` if
    any exists, else `clients.openWindow(data.target_path)`.
  - **No** offline shell, no fetch interception, no message-event
    handlers for cross-tab coordination. This file does exactly two
    things.
  - Served at the root path `/sw.js` to claim scope `/`. A dedicated
    Axum route handler is required because static assets live under
    `/assets/`, which would limit the SW scope to `/assets/`.
- **Page-side registration JS.** Lives inline in `templates/layout.html`,
  inside the existing notification-bus IIFE that processes `Mentioned`
  events. ~30 additional lines. Triggers:
  - On the first Mentioned event handled while
    `cfg.dataset.pushEnabled === '1'` AND `Notification.permission ===
    'granted'`, the IIFE fetches `/push/vapid-public-key`, registers
    `/sw.js` via `navigator.serviceWorker.register('/sw.js', { scope: '/' })`,
    calls `getSubscription()` first (idempotent), and if no existing
    subscription returns one, calls
    `pushManager.subscribe({ userVisibleOnly: true, applicationServerKey:
    <vapid bytes> })`, then POSTs `endpoint`, `keys.p256dh`, and
    `keys.auth` (all base64url) to `/push/subscribe` as JSON.
  - The result (success or any failure) is cached on `window.__lcPushTried`
    so subsequent Mentioned events don't repeat the work.
  - Errors are caught and logged to `console.warn`. They do not interrupt
    the rest of the bus processing.
- **Settings UI.** Third checkbox under "Notifications" in
  `templates/settings/page.html`:

  ```html
  <input type="checkbox" name="notify_push_enabled" value="1"
         {% if user.notify_push_enabled %}checked{% endif %}
         {% if !push_available %}disabled{% endif %}>
  ```

  Help text is conditional on `push_available`: when `false`, the label
  reads "(unavailable - server is not configured for push)" so an admin
  who set up the server without `LETS_CHAT_SECRET_KEY` understands why
  the box is greyed. `push_available` is a new bool field on
  `UserSettingsPage`, populated as `state.vapid.is_some()`.
- **Payload shape** (`push::payload::build`):

  ```json
  {
    "title": "Alice in #general",   // or "Alice (DM)" for kind == "dm"
    "body":  "<snippet, <=140 chars>",
    "icon":  "/assets/notification-icon.png?v=<asset_version>",
    "tag":   "lc-<room_id>",
    "data":  { "target_path": "/room/<id>" }
  }
  ```

  `tag` collapses repeat Pushes into a single OS notification per room
  (Slack/Discord behavior). `icon` points at a 192x192 PNG (added in
  Task 1) because SVG icons render inconsistently across Chrome and
  Firefox. Snippet matches the same `build_snippet` helper that Phase 14
  uses for the in-app notification surface.

## Tech Stack

- **New crates:**
  - `web-push = "0.10"` - VAPID JWT signing, AES-128-GCM payload
    encryption per RFC 8291, third-party Push service HTTP client.
    Tokio async, isahc-based HTTP. Maintained on crates.io.
  - `p256 = { version = "0.13", features = ["pkcs8", "pem"] }` - generate
    the ECDSA P-256 keypair at first boot, encode private key as
    PKCS#8 PEM and public key as raw uncompressed point bytes for the
    `applicationServerKey` field.
  - `bytes = "1"` - the `Bytes` type already enters via transitive deps
    but pin it directly so the trait signature is stable.
- `aes-gcm` is already a direct dep (used for TOTP). Reused.
- `async-trait` is already a direct dep. Reused.
- `serde_json` is already a workspace dep. Reused for payload building.
- **New static assets:**
  - `server/assets/sw.js` - service worker, served via dedicated route
    handler at `/sw.js`.
  - `server/assets/notification-icon.png` - 192x192 PNG, used as the
    `icon` field in Push payloads. Generated from the existing
    `favicon.svg` (placeholder OK; document the swap in Task 1).
- **No build steps changed.** Tailwind, Bun, just recipes are untouched.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Add | `server/migrations/auth/0009_push_subscriptions.sql` | `push_subscriptions` table + `notify_push_enabled` column on `users`. |
| Add | `server/migrations/settings/0003_vapid_keypair.sql` | Singleton `vapid_keypair` table for the encrypted VAPID private key + plaintext public key. |
| Edit | `server/Cargo.toml` | Add `web-push`, `p256`, `bytes`. |
| Edit | `server/src/models/user.rs` | Add `notify_push_enabled: bool` to `UserRecord` and `User`. |
| Edit | `server/src/db/auth.rs` | Add `notify_push_enabled` to every `UserRecord` SELECT and the `User` mapping; extend `set_notification_prefs` to take a third `push: bool` param. |
| Add | `server/src/db/push_subscriptions.rs` | `PushSubscription` row type + `insert_or_replace`, `for_user`, `delete_by_endpoint`, `bump_last_seen`. |
| Add | `server/src/db/vapid.rs` | `VapidKeypair` (decrypted, in-memory) + `load_or_generate(pool, secret_key)` + serialization helpers. |
| Edit | `server/src/db/mod.rs` | `pub mod push_subscriptions;` + `pub mod vapid;`. |
| Add | `server/src/push/mod.rs` | `PushClient` trait + `IsahcPushClient` + `MockPushClient` + `dispatch(...)` helper. |
| Add | `server/src/push/payload.rs` | `build(event) -> Bytes` constructing the JSON Push payload from `ChatEvent::Mentioned`. |
| Edit | `server/src/lib.rs` | `pub mod push;`. |
| Edit | `server/src/state.rs` | Add `vapid: Option<Arc<VapidKeypair>>` and `push_client: Arc<dyn push::PushClient>` to `AppState`. |
| Edit | `server/src/main.rs` | Initialize `vapid` (auto-generate when secret key is set, else `None`); construct the `IsahcPushClient` and store as `Arc<dyn PushClient>`. |
| Add | `server/src/routes/push.rs` | `GET /sw.js`, `GET /push/vapid-public-key`, `POST /push/subscribe`. |
| Edit | `server/src/routes/mod.rs` | `mod push;` + register the three routes. |
| Edit | `server/src/routes/room.rs` | `post_message` and `patch_message` invoke `push::dispatch` after each `Hub::broadcast_to_user(... Mentioned ...)` call. |
| Edit | `server/src/routes/settings.rs` | `SettingsForm.notify_push_enabled` + pass `push` into `set_notification_prefs`. |
| Edit | `server/src/views/settings.rs` | `UserSettingsPage.push_available: bool` + thread through. |
| Edit | `server/templates/settings/page.html` | Third checkbox under "Notifications", with disabled state + help text. |
| Edit | `server/templates/layout.html` | `lc-mention-counts` div gains `data-push-enabled` + Push registration block inside the existing IIFE. |
| Add | `server/assets/sw.js` | Service worker: `push` (with visibility suppression) + `notificationclick`. |
| Add | `server/assets/notification-icon.png` | 192x192 PNG icon. |
| Add | `server/tests/db_push_subscriptions.rs` | DB CRUD tests. |
| Add | `server/tests/db_vapid.rs` | VAPID generate-and-load round-trip + idempotent reload. |
| Add | `server/tests/push_dispatch.rs` | Fan-out integration test using `MockPushClient`: verifies `Mentioned` triggers send, mute filtering blocks, DM bypass, 410 deletes the row. |
| Edit | every `tests/*.rs` that opens auth/settings pools | Add the new migration files to the migration include list. |

## Tasks

### Task 1 - Schema, deps, model fields, asset

- [ ] Confirm next migration numbers:
      `ls server/migrations/auth/` -> next is **`0009`**;
      `ls server/migrations/settings/` -> next is **`0003`**;
      `ls server/migrations/chat/` -> unchanged (this phase touches no chat
      migrations, despite the prompt's "chat/0016" wording - VAPID is a
      settings concern). The deviation is recorded in the summary.
- [ ] Edit `server/Cargo.toml`. Add to `[dependencies]`:

```toml
web-push = "0.10"
p256 = { version = "0.13", features = ["pkcs8", "pem"] }
bytes = "1"
```

  Confirm `aes-gcm`, `async-trait`, `serde_json` are already present
  (they are).

- [ ] `git checkout -b feat/web-push`
- [ ] Create `server/migrations/auth/0009_push_subscriptions.sql`:

```sql
CREATE TABLE IF NOT EXISTS push_subscriptions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       TEXT    NOT NULL,
    endpoint      TEXT    NOT NULL UNIQUE,
    p256dh_key    TEXT    NOT NULL,
    auth_key      TEXT    NOT NULL,
    user_agent    TEXT,
    created_at    TEXT    NOT NULL DEFAULT (datetime('now')),
    last_seen_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_push_subscriptions_user
    ON push_subscriptions (user_id);

ALTER TABLE users ADD COLUMN notify_push_enabled INTEGER NOT NULL DEFAULT 0;
```

  The unique constraint on `endpoint` matches the schema design above.
  `notify_push_enabled` defaults to `0` (opt-in).

- [ ] Create `server/migrations/settings/0003_vapid_keypair.sql`:

```sql
CREATE TABLE IF NOT EXISTS vapid_keypair (
    id                         INTEGER PRIMARY KEY CHECK (id = 1),
    public_key_b64url          TEXT NOT NULL,
    private_key_pem_encrypted  BLOB NOT NULL,
    private_key_pem_nonce      BLOB NOT NULL,
    created_at                 TEXT NOT NULL DEFAULT (datetime('now'))
);
```

  The `CHECK (id = 1)` constraint enforces the singleton-row invariant
  at the schema level; `db::vapid` always reads/writes `id = 1`.

- [ ] Edit `server/src/models/user.rs`. Add `pub notify_push_enabled: bool`
      to both `UserRecord` and `User`, and update the `From<UserRecord>
      for User` mapping to copy the field through. Place the new field
      adjacent to the existing `notify_browser_enabled` /
      `notify_sound_enabled` pair on each struct.

- [ ] Edit `server/src/db/auth.rs`. Every existing SELECT that lists user
      columns (lines 36, 57, 262, 298, 570, 712 per the Phase 15 audit)
      gains `notify_push_enabled` next to the existing two. The mapping
      that builds `UserRecord` rows
      (`notify_browser_enabled: r.get::<i64, _>(...) != 0` style) gains a
      parallel line for `notify_push_enabled`.

      Update `set_notification_prefs` to accept a third bool:

```rust
pub async fn set_notification_prefs(
    pool: &SqlitePool,
    user_id: &str,
    browser: bool,
    sound: bool,
    push: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET notify_browser_enabled = ?, \
                          notify_sound_enabled   = ?, \
                          notify_push_enabled    = ? \
         WHERE id = ?",
    )
    .bind(browser as i64)
    .bind(sound as i64)
    .bind(push as i64)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}
```

      Audit the only caller (`server/src/routes/settings.rs`) and update
      it; that call site is rewritten in Task 7.

- [ ] Add the placeholder asset
      `server/assets/notification-icon.png`. A 192x192 PNG suffices.
      Quick path: copy the existing `favicon.svg` and rasterize via:

```nu
docker run --rm --volume $env.PWD:/work --workdir /work alpine:3.20 sh -c "apk add --no-cache imagemagick > /dev/null && convert -background none -resize 192x192 server/assets/favicon.svg server/assets/notification-icon.png"
```

      If asset generation is annoying or the favicon does not rasterize
      cleanly, drop a 192x192 1-color PNG placeholder and add a TODO
      comment in the plan summary noting "swap in a real icon before
      release."

- [ ] Update test setup helpers. Tests that include
      `migrations/auth/0008_two_factor.sql` must append
      `0009_push_subscriptions.sql`; tests that include
      `migrations/settings/0002_uploads.sql` must append
      `0003_vapid_keypair.sql`.

      Affected files (confirmed via
      `grep -lE "include_str!\(\"\.\./migrations/auth" server/tests/*.rs`
      and the parallel for settings):

  - `server/tests/db_auth.rs` - append the new `migration9 =
    include_str!("../migrations/auth/0009_push_subscriptions.sql");`
    block + `sqlx::raw_sql(migration9).execute(...)` line.
  - `server/tests/db_dm.rs`, `db_enclave.rs`, `db_invite.rs`,
    `db_mentions.rs`, `db_moderation.rs`, `db_notifications.rs`,
    `db_private_rooms.rs`, `db_reactions.rs`, `db_read_receipts.rs`,
    `db_search.rs`, `db_settings.rs`, `db_status.rs`, `db_two_factor.rs`,
    `db_uploads.rs`, `last_visited.rs`, `message_editing.rs`,
    `message_grouping.rs`, `migration_enclaves.rs`, `perms.rs`,
    `rbac.rs`, `routes_enclave.rs`, `routes_mentions.rs`,
    `routes_room_mute.rs`, `routes_uploads.rs` - audit each. Append the
    new auth migration to any auth-pool include list and the new
    settings migration to any settings-pool include list.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server` (the build should pass; new
      column means existing user-related tests that build users pass
      `notify_push_enabled: false` through the From mapping
      automatically).
- [ ] `git add server/migrations/auth/0009_push_subscriptions.sql server/migrations/settings/0003_vapid_keypair.sql server/Cargo.toml server/Cargo.lock server/src/models/user.rs server/src/db/auth.rs server/assets/notification-icon.png server/tests/`

### Task 2 - VAPID keypair: generate, encrypt, persist, load

- [ ] Create `server/src/db/vapid.rs`:

```rust
//! VAPID keypair persistence for Web Push.
//!
//! On first boot the server generates an ECDSA P-256 keypair, AES-256-GCM
//! encrypts the PKCS#8 PEM-encoded private key under the process secret
//! key, and writes the singleton row of `vapid_keypair`. On every
//! subsequent boot, the row is loaded and decrypted.
//!
//! Push is disabled entirely when no secret key is configured; this
//! module is therefore only ever called when `secret_key` is `Some`.

use p256::pkcs8::{EncodePrivateKey, EncodePublicKey};
use sqlx::{Row, SqlitePool};

use crate::crypto;

pub struct VapidKeypair {
    /// Raw uncompressed P-256 point (65 bytes, leading 0x04), base64url-
    /// encoded. Sent to the page as the `applicationServerKey` value.
    pub public_key_b64url: String,
    /// PKCS#8 PEM-encoded private key. Held in memory after decryption;
    /// passed verbatim to `web_push::VapidSignatureBuilder::from_pem`.
    pub private_key_pem: String,
}

#[derive(Debug, thiserror::Error)]
pub enum VapidError {
    #[error("crypto: {0}")]
    Crypto(#[from] crypto::CryptoError),
    #[error("sql: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("p256 key encoding")]
    KeyEncoding,
}

/// Returns the persisted keypair, generating + persisting one on first call.
/// Idempotent: subsequent calls observe the existing row and return the
/// same keypair.
pub async fn load_or_generate(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
) -> Result<VapidKeypair, VapidError> {
    if let Some(kp) = load(pool, secret_key).await? {
        return Ok(kp);
    }
    let kp = generate()?;
    persist(pool, secret_key, &kp).await?;
    Ok(kp)
}

async fn load(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
) -> Result<Option<VapidKeypair>, VapidError> {
    let row = sqlx::query(
        "SELECT public_key_b64url, private_key_pem_encrypted, private_key_pem_nonce \
           FROM vapid_keypair WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let public_key_b64url: String = r.get("public_key_b64url");
    let encrypted: Vec<u8> = r.get("private_key_pem_encrypted");
    let nonce: Vec<u8> = r.get("private_key_pem_nonce");
    let pem_bytes = crypto::open(secret_key, &nonce, &encrypted)?;
    let private_key_pem = String::from_utf8(pem_bytes).map_err(|_| VapidError::KeyEncoding)?;
    Ok(Some(VapidKeypair {
        public_key_b64url,
        private_key_pem,
    }))
}

async fn persist(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
    kp: &VapidKeypair,
) -> Result<(), VapidError> {
    let (encrypted, nonce) = crypto::seal(secret_key, kp.private_key_pem.as_bytes())?;
    sqlx::query(
        "INSERT INTO vapid_keypair \
             (id, public_key_b64url, private_key_pem_encrypted, private_key_pem_nonce) \
         VALUES (1, ?, ?, ?) \
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(&kp.public_key_b64url)
    .bind(&encrypted)
    .bind(&nonce)
    .execute(pool)
    .await?;
    Ok(())
}

/// Generate a fresh ECDSA P-256 keypair. Returns the public key as a
/// base64url-encoded uncompressed point (the `applicationServerKey`
/// format expected by `PushManager.subscribe`) and the private key as
/// PKCS#8 PEM (the format `web_push::VapidSignatureBuilder::from_pem`
/// expects).
fn generate() -> Result<VapidKeypair, VapidError> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    let signing = p256::SecretKey::random(&mut rand::thread_rng());
    let public_point = signing.public_key().to_encoded_point(false); // 65 bytes uncompressed
    let public_key_b64url = URL_SAFE_NO_PAD.encode(public_point.as_bytes());
    let private_key_pem = signing
        .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
        .map_err(|_| VapidError::KeyEncoding)?
        .to_string();
    Ok(VapidKeypair {
        public_key_b64url,
        private_key_pem,
    })
}
```

      `base64` is not currently a direct dep; add it to `Cargo.toml`:

```toml
base64 = "0.22"
```

      Note: if `web-push` 0.10 already pulls in a compatible `base64`,
      use that version; otherwise pin `0.22`. Confirm in `cargo tree` if
      needed.

- [ ] Add `pub mod vapid;` to `server/src/db/mod.rs`.

- [ ] `./dev/cargo check -p lets-chat-server`

- [ ] Create `server/tests/db_vapid.rs`:

```rust
use lets_chat::crypto;
use lets_chat::db::vapid::{self, VapidKeypair};
use sqlx::SqlitePool;

async fn setup_settings_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/settings/0001_create_tables.sql"),
        include_str!("../migrations/settings/0002_uploads.sql"),
        include_str!("../migrations/settings/0003_vapid_keypair.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

fn test_secret_key() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"vapid-test-key");
    let out = h.finalize();
    let mut k = [0u8; 32];
    k.copy_from_slice(&out);
    k
}

#[tokio::test]
async fn first_call_generates_and_persists() {
    let pool = setup_settings_pool().await;
    let key = test_secret_key();
    let kp = vapid::load_or_generate(&pool, &key).await.unwrap();
    // Public key is a 65-byte uncompressed P-256 point: base64url-decoded
    // length is 65.
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&kp.public_key_b64url)
        .unwrap();
    assert_eq!(raw.len(), 65);
    assert_eq!(raw[0], 0x04);
    assert!(kp.private_key_pem.contains("BEGIN PRIVATE KEY"));
}

#[tokio::test]
async fn second_call_returns_persisted_keypair() {
    let pool = setup_settings_pool().await;
    let key = test_secret_key();
    let first = vapid::load_or_generate(&pool, &key).await.unwrap();
    let second = vapid::load_or_generate(&pool, &key).await.unwrap();
    assert_eq!(first.public_key_b64url, second.public_key_b64url);
    assert_eq!(first.private_key_pem, second.private_key_pem);
}

#[tokio::test]
async fn private_key_is_not_stored_plaintext() {
    let pool = setup_settings_pool().await;
    let key = test_secret_key();
    let kp = vapid::load_or_generate(&pool, &key).await.unwrap();
    let row: (Vec<u8>,) = sqlx::query_as(
        "SELECT private_key_pem_encrypted FROM vapid_keypair WHERE id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let needle = b"BEGIN PRIVATE KEY";
    assert!(
        !row.0.windows(needle.len()).any(|w| w == needle),
        "encrypted blob should not contain the PEM marker"
    );
    let _ = kp; // keep used
}

#[tokio::test]
async fn wrong_key_fails_to_decrypt() {
    let pool = setup_settings_pool().await;
    let key = test_secret_key();
    let _ = vapid::load_or_generate(&pool, &key).await.unwrap();
    let mut wrong = key;
    wrong[0] ^= 0xff;
    assert!(vapid::load_or_generate(&pool, &wrong).await.is_err());
}
```

      `crypto::CryptoError` is already exposed at `crate::crypto`; the
      tests use the public re-export. The `base64::engine::...` import
      mirrors the implementation.

- [ ] `./dev/cargo test -p lets-chat-server --test db_vapid`
- [ ] `git add server/src/db/vapid.rs server/src/db/mod.rs server/tests/db_vapid.rs server/Cargo.toml server/Cargo.lock`

### Task 3 - `db::push_subscriptions` module + tests

- [ ] Create `server/src/db/push_subscriptions.rs`:

```rust
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct PushSubscription {
    pub id: i64,
    pub user_id: String,
    pub endpoint: String,
    pub p256dh_key: String,
    pub auth_key: String,
    pub user_agent: Option<String>,
}

/// Insert a subscription if its `endpoint` is unseen, else replace the
/// owning user (and refresh the keys + user_agent). Endpoint identifies
/// the (browser, application server) pair, so a second user logging in
/// on the same browser inherits the row.
pub async fn insert_or_replace(
    pool: &SqlitePool,
    user_id: &str,
    endpoint: &str,
    p256dh_key: &str,
    auth_key: &str,
    user_agent: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query(
        "INSERT INTO push_subscriptions \
             (user_id, endpoint, p256dh_key, auth_key, user_agent) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(endpoint) DO UPDATE SET \
             user_id     = excluded.user_id, \
             p256dh_key  = excluded.p256dh_key, \
             auth_key    = excluded.auth_key, \
             user_agent  = excluded.user_agent, \
             last_seen_at = datetime('now') \
         RETURNING id",
    )
    .bind(user_id)
    .bind(endpoint)
    .bind(p256dh_key)
    .bind(auth_key)
    .bind(user_agent)
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

pub async fn for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<PushSubscription>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, user_id, endpoint, p256dh_key, auth_key, user_agent \
           FROM push_subscriptions WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PushSubscription {
            id: r.get("id"),
            user_id: r.get("user_id"),
            endpoint: r.get("endpoint"),
            p256dh_key: r.get("p256dh_key"),
            auth_key: r.get("auth_key"),
            user_agent: r.get("user_agent"),
        })
        .collect())
}

pub async fn delete_by_endpoint(
    pool: &SqlitePool,
    endpoint: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = ?")
        .bind(endpoint)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn bump_last_seen(
    pool: &SqlitePool,
    endpoint: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE push_subscriptions SET last_seen_at = datetime('now') \
          WHERE endpoint = ?",
    )
    .bind(endpoint)
    .execute(pool)
    .await?;
    Ok(())
}
```

- [ ] Add `pub mod push_subscriptions;` to `server/src/db/mod.rs`.

- [ ] Create `server/tests/db_push_subscriptions.rs`:

```rust
use lets_chat::db::push_subscriptions::{
    self, delete_by_endpoint, for_user, insert_or_replace, PushSubscription,
};
use sqlx::SqlitePool;

async fn setup_auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/auth/0001_create_tables.sql"),
        include_str!("../migrations/auth/0002_read_receipts.sql"),
        include_str!("../migrations/auth/0003_profile_fields.sql"),
        include_str!("../migrations/auth/0004_user_status.sql"),
        include_str!("../migrations/auth/0005_profile_visibility.sql"),
        include_str!("../migrations/auth/0006_user_blocks.sql"),
        include_str!("../migrations/auth/0007_notification_settings.sql"),
        include_str!("../migrations/auth/0008_two_factor.sql"),
        include_str!("../migrations/auth/0009_push_subscriptions.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn insert_persists_a_row() {
    let pool = setup_auth_pool().await;
    let id = insert_or_replace(&pool, "u1", "https://endpoint.example/abc", "p256", "auth", Some("ua"))
        .await
        .unwrap();
    assert!(id > 0);
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].endpoint, "https://endpoint.example/abc");
    assert_eq!(subs[0].p256dh_key, "p256");
}

#[tokio::test]
async fn endpoint_conflict_replaces_owning_user() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "https://endpoint.example/abc", "p1", "a1", None)
        .await
        .unwrap();
    insert_or_replace(&pool, "u2", "https://endpoint.example/abc", "p2", "a2", None)
        .await
        .unwrap();
    assert!(for_user(&pool, "u1").await.unwrap().is_empty());
    let subs = for_user(&pool, "u2").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].p256dh_key, "p2");
    assert_eq!(subs[0].auth_key, "a2");
}

#[tokio::test]
async fn for_user_lists_all_devices() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "https://e1", "p", "a", None).await.unwrap();
    insert_or_replace(&pool, "u1", "https://e2", "p", "a", None).await.unwrap();
    insert_or_replace(&pool, "u2", "https://e3", "p", "a", None).await.unwrap();
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 2);
}

#[tokio::test]
async fn delete_by_endpoint_removes_one_row() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "https://e1", "p", "a", None).await.unwrap();
    insert_or_replace(&pool, "u1", "https://e2", "p", "a", None).await.unwrap();
    delete_by_endpoint(&pool, "https://e1").await.unwrap();
    let subs = for_user(&pool, "u1").await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].endpoint, "https://e2");
}

#[tokio::test]
async fn bump_last_seen_updates_timestamp() {
    let pool = setup_auth_pool().await;
    insert_or_replace(&pool, "u1", "https://e1", "p", "a", None).await.unwrap();
    let before: String = sqlx::query_scalar(
        "SELECT last_seen_at FROM push_subscriptions WHERE endpoint = ?",
    )
    .bind("https://e1")
    .fetch_one(&pool)
    .await
    .unwrap();
    // SQLite second-resolution; sleep so the bump observably differs.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    push_subscriptions::bump_last_seen(&pool, "https://e1").await.unwrap();
    let after: String = sqlx::query_scalar(
        "SELECT last_seen_at FROM push_subscriptions WHERE endpoint = ?",
    )
    .bind("https://e1")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(before, after);
}
```

- [ ] `./dev/cargo test -p lets-chat-server --test db_push_subscriptions`
- [ ] `git add server/src/db/push_subscriptions.rs server/src/db/mod.rs server/tests/db_push_subscriptions.rs`

### Task 4 - `push` module: trait, impls, payload, dispatch

- [ ] Create `server/src/push/mod.rs`:

```rust
//! Web Push fan-out for `Mentioned` events.
//!
//! Public surface:
//! - `PushClient` trait: one method, `send`. Production wraps
//!   `web_push::IsahcWebPushClient`; tests substitute `MockPushClient`.
//! - `dispatch`: the helper invoked from each `Mentioned`-broadcast site.
//!   Performs the mute/notify-push gating and spawns one fire-and-forget
//!   task per stored subscription.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;

use crate::db::{
    self,
    notifications::MuteMode,
    push_subscriptions::PushSubscription,
};
use crate::state::AppState;
use crate::ws::events::ChatEvent;

pub mod payload;

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("endpoint gone (HTTP 404 / 410): {0}")]
    EndpointGone(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("encrypt: {0}")]
    Encrypt(String),
}

#[async_trait]
pub trait PushClient: Send + Sync {
    async fn send(&self, sub: &PushSubscription, payload: Bytes) -> Result<(), PushError>;
}

/// Production `PushClient` backed by `web_push::IsahcWebPushClient`.
/// Holds an `Arc` of the in-memory VAPID keypair so the JWT signature is
/// freshly built per send (the underlying signature builder is cheap;
/// caching it across sends is a future optimization).
pub struct IsahcPushClient {
    inner: web_push::IsahcWebPushClient,
    private_key_pem: Arc<String>,
    contact: String,
}

impl IsahcPushClient {
    pub fn new(private_key_pem: Arc<String>, contact: String) -> Self {
        Self {
            inner: web_push::IsahcWebPushClient::new()
                .expect("web-push client construction"),
            private_key_pem,
            contact,
        }
    }
}

#[async_trait]
impl PushClient for IsahcPushClient {
    async fn send(&self, sub: &PushSubscription, payload: Bytes) -> Result<(), PushError> {
        use web_push::{
            ContentEncoding, SubscriptionInfo, SubscriptionKeys,
            VapidSignatureBuilder, WebPushClient, WebPushError, WebPushMessageBuilder,
        };

        let info = SubscriptionInfo {
            endpoint: sub.endpoint.clone(),
            keys: SubscriptionKeys {
                p256dh: sub.p256dh_key.clone(),
                auth: sub.auth_key.clone(),
            },
        };
        let mut builder = WebPushMessageBuilder::new(&info);
        builder.set_payload(ContentEncoding::Aes128Gcm, &payload);
        let sig = VapidSignatureBuilder::from_pem(self.private_key_pem.as_bytes(), &info)
            .and_then(|b| b.add_claim("sub", self.contact.as_str()).build())
            .map_err(|e| PushError::Encrypt(format!("vapid: {e}")))?;
        builder.set_vapid_signature(sig);
        let msg = builder
            .build()
            .map_err(|e| PushError::Encrypt(format!("encrypt: {e}")))?;
        match self.inner.send(msg).await {
            Ok(()) => Ok(()),
            Err(WebPushError::EndpointNotValid) | Err(WebPushError::EndpointNotFound) => {
                Err(PushError::EndpointGone(sub.endpoint.clone()))
            }
            Err(e) => Err(PushError::Transport(e.to_string())),
        }
    }
}

/// Test-only `PushClient`. Records every `send` for assertion.
#[derive(Default)]
pub struct MockPushClient {
    pub sent: tokio::sync::Mutex<Vec<RecordedSend>>,
}

#[derive(Debug, Clone)]
pub struct RecordedSend {
    pub endpoint: String,
    pub user_id: String,
    pub payload: Bytes,
}

#[async_trait]
impl PushClient for MockPushClient {
    async fn send(&self, sub: &PushSubscription, payload: Bytes) -> Result<(), PushError> {
        self.sent.lock().await.push(RecordedSend {
            endpoint: sub.endpoint.clone(),
            user_id: sub.user_id.clone(),
            payload,
        });
        Ok(())
    }
}

/// Fan out a single `Mentioned`-equivalent Push to every registered
/// subscription for `recipient_user_id`. Honors:
///   1. global Push availability (`state.vapid` is some)
///   2. per-user `notify_push_enabled`
///   3. per-room mute mode (DM kind bypasses the room check)
///
/// Each subscription send runs as its own `tokio::spawn` task. Failures
/// are logged at warn level. 410-Gone deletes the row inline.
pub async fn dispatch(state: &AppState, recipient_user_id: &str, event: &ChatEvent) {
    if state.vapid.is_none() {
        return;
    }
    let ChatEvent::Mentioned {
        kind, room_id, ..
    } = event else {
        return; // dispatch is only ever called with Mentioned today
    };

    // Per-user gate.
    let recipient = match db::auth::find_user_by_id(&state.auth, recipient_user_id).await {
        Ok(Some(u)) => u,
        Ok(None) | Err(_) => return,
    };
    if !recipient.notify_push_enabled {
        return;
    }

    // Per-room mute. DM kind bypasses (DM mute is a future phase).
    if kind != "dm" {
        // FUTURE: when the DM-mute phase lands, this bypass becomes
        // conditional on dm_mute_state(user, peer).
        let mode = db::notifications::room_mute_mode(&state.chat, recipient_user_id, *room_id)
            .await
            .unwrap_or(MuteMode::None);
        if matches!(mode, MuteMode::All) {
            return;
        }
        // MuteMode::ExceptMentions falls through; only Mentioned events
        // reach this helper, so the gate matches the WS path's behavior.
    }

    let subs = match db::push_subscriptions::for_user(&state.auth, recipient_user_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "push: subscription lookup failed");
            return;
        }
    };
    if subs.is_empty() {
        return;
    }

    let payload = match payload::build(event) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "push: payload build failed");
            return;
        }
    };
    for sub in subs {
        let client = state.push_client.clone();
        let auth_pool = state.auth.clone();
        let payload = payload.clone();
        tokio::spawn(async move {
            match client.send(&sub, payload).await {
                Ok(()) => {
                    let _ = db::push_subscriptions::bump_last_seen(&auth_pool, &sub.endpoint).await;
                }
                Err(PushError::EndpointGone(_)) => {
                    let _ = db::push_subscriptions::delete_by_endpoint(&auth_pool, &sub.endpoint).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, endpoint = %sub.endpoint, "push send failed");
                }
            }
        });
    }
}
```

- [ ] Create `server/src/push/payload.rs`:

```rust
use bytes::Bytes;

use crate::ws::events::ChatEvent;

#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    #[error("not a Mentioned event")]
    WrongEvent,
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Build the Push payload JSON for a `Mentioned` event. Matches the
/// shape consumed by `server/assets/sw.js`'s `push` handler.
pub fn build(event: &ChatEvent) -> Result<Bytes, PayloadError> {
    let ChatEvent::Mentioned {
        kind,
        room_id,
        room_label,
        author_label,
        snippet,
        target_path,
        ..
    } = event
    else {
        return Err(PayloadError::WrongEvent);
    };
    let title = if kind == "dm" {
        format!("{author_label} (DM)")
    } else {
        format!("{author_label} in {room_label}")
    };
    let value = serde_json::json!({
        "title": title,
        "body":  snippet,
        "icon":  "/assets/notification-icon.png",
        "tag":   format!("lc-{room_id}"),
        "data":  { "target_path": target_path },
    });
    Ok(Bytes::from(serde_json::to_vec(&value)?))
}
```

- [ ] Edit `server/src/lib.rs`. Add `pub mod push;` next to the existing
      `pub mod` declarations.

- [ ] `./dev/cargo check -p lets-chat-server`

- [ ] `git add server/src/push/mod.rs server/src/push/payload.rs server/src/lib.rs`

### Task 5 - `AppState` wiring + main.rs initialization

- [ ] Edit `server/src/state.rs`. Add the two new fields:

```rust
use std::sync::Arc;

use sqlx::SqlitePool;

use crate::db::vapid::VapidKeypair;
use crate::push::PushClient;
use crate::ws::hub::Hub;

#[derive(Clone)]
pub struct AppState {
    pub auth: SqlitePool,
    pub chat: SqlitePool,
    pub settings: SqlitePool,
    pub hub: Arc<Hub>,
    pub asset_version: String,
    pub secret_key: Option<Arc<[u8; 32]>>,
    /// `Some` when `LETS_CHAT_SECRET_KEY` is set AND the VAPID keypair
    /// has been generated/loaded. `None` disables Push entirely (no
    /// subscribe route, no fan-out, settings checkbox shows disabled).
    pub vapid: Option<Arc<VapidKeypair>>,
    /// Always present. When `vapid` is `None`, the dispatch helper
    /// short-circuits before any client method is called, so a no-op
    /// implementation isn't required - the production `IsahcPushClient`
    /// is constructed regardless.
    pub push_client: Arc<dyn PushClient>,
}

impl AppState {
    pub fn two_factor_available(&self) -> bool {
        self.secret_key.is_some()
    }
    pub fn push_available(&self) -> bool {
        self.vapid.is_some()
    }
}
```

- [ ] Edit `server/src/main.rs`. After loading `secret_key`, generate or
      load the VAPID keypair and construct the production
      `IsahcPushClient`:

```rust
let secret_key = lets_chat::crypto::load_secret_key_from_env().map(std::sync::Arc::new);

let vapid = if let Some(ref key) = secret_key {
    match lets_chat::db::vapid::load_or_generate(&settings_pool, key.as_ref()).await {
        Ok(kp) => Some(std::sync::Arc::new(kp)),
        Err(e) => {
            tracing::warn!(error = %e, "vapid keypair load failed; push disabled");
            None
        }
    }
} else {
    None
};

let push_client: std::sync::Arc<dyn lets_chat::push::PushClient> = match &vapid {
    Some(kp) => std::sync::Arc::new(lets_chat::push::IsahcPushClient::new(
        std::sync::Arc::new(kp.private_key_pem.clone()),
        std::env::var("LETS_CHAT_PUSH_CONTACT")
            .unwrap_or_else(|_| "mailto:admin@localhost".to_string()),
    )),
    None => std::sync::Arc::new(lets_chat::push::IsahcPushClient::new(
        std::sync::Arc::new(String::new()),
        "mailto:admin@localhost".to_string(),
    )),
};

let state = AppState {
    auth: auth_pool,
    chat: chat_pool,
    settings: settings_pool,
    hub: std::sync::Arc::new(Hub::new()),
    asset_version: compute_asset_version(),
    secret_key,
    vapid,
    push_client,
};
```

      Note the `LETS_CHAT_PUSH_CONTACT` env var: VAPID requires a `sub`
      claim in the JWT, typically `mailto:` to identify the application
      operator to the Push service. Default is fine for self-hosted; ops
      can override. This is not the same as a VAPID env-var override
      (which is out of scope) - it's just the contact identity.

      Pull the pool initialization out of the inline `AppState { ... }`
      literal so the new `vapid` step has a `settings_pool` binding to
      reference. Existing pool inits become bare bindings:

```rust
let auth_pool = db::open_auth_pool().await;
let chat_pool = db::open_chat_pool().await;
let settings_pool = db::open_settings_pool().await;
```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo check -p lets-chat-server --no-default-features --features saas`
- [ ] `./dev/cargo test -p lets-chat-server` (existing tests should still
      compile; any test that constructs `AppState` directly needs to set
      `vapid: None` and `push_client: Arc::new(MockPushClient::default())`).
- [ ] `git add server/src/state.rs server/src/main.rs`

### Task 6 - Service worker + VAPID public key route + sw.js route

- [ ] Create `server/assets/sw.js`. Hard cap: 80 lines including
      comments and blank lines. The visibility-suppression branch is
      mandatory.

```javascript
// lets-chat service worker.
//
// Two responsibilities, nothing else:
//   1. push:              parse JSON payload, render OS notification.
//   2. notificationclick: focus or open the target tab.
//
// IMPORTANT: the push handler suppresses showNotification() when any
// visible client is already on data.target_path. This is *baked in*, not
// optional - the user is actively reading the room, an OS-level ping
// would be noise. If a future tweak wants to relax this, do so behind an
// explicit user setting.

self.addEventListener('install', () => {
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener('push', (event) => {
  if (!event.data) return;
  let payload;
  try { payload = event.data.json(); } catch (e) { return; }
  const target = (payload.data && payload.data.target_path) || '/';
  event.waitUntil((async () => {
    const visible = await self.clients.matchAll({ type: 'window', visible: true });
    const onTarget = visible.some((c) => {
      try { return new URL(c.url).pathname === target; } catch (e) { return false; }
    });
    if (onTarget) return; // user is already reading the room
    return self.registration.showNotification(payload.title || 'lets-chat', {
      body: payload.body || '',
      icon: payload.icon,
      tag: payload.tag,
      data: payload.data || {},
    });
  })());
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const target = (event.notification.data && event.notification.data.target_path) || '/';
  event.waitUntil((async () => {
    const all = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    for (const c of all) {
      try {
        if (new URL(c.url).pathname === target) {
          await c.focus();
          return;
        }
      } catch (e) {}
    }
    await self.clients.openWindow(target);
  })());
});
```

- [ ] Create `server/src/routes/push.rs`:

```rust
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;

/// GET /sw.js - serve the service worker from root scope. Must NOT live
/// under /assets/ because the SW's registration scope is bounded by its
/// own URL (so /assets/sw.js could only claim /assets/, not the whole
/// app).
pub async fn get_service_worker() -> Response {
    let body = include_str!("../../assets/sw.js");
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        body,
    )
        .into_response()
}

/// GET /push/vapid-public-key - return the base64url-encoded raw P-256
/// public key. The page-side JS uses this as `applicationServerKey`
/// when calling `pushManager.subscribe`. 404 when push is disabled.
pub async fn get_vapid_public_key(State(state): State<AppState>) -> Result<Response, AppError> {
    let Some(kp) = state.vapid.as_ref() else {
        return Err(AppError::NotFound);
    };
    Ok(Json(serde_json::json!({
        "key": kp.public_key_b64url
    }))
    .into_response())
}

#[derive(Deserialize)]
pub struct SubscribeBody {
    pub endpoint: String,
    pub keys: SubscribeKeys,
}

#[derive(Deserialize)]
pub struct SubscribeKeys {
    pub p256dh: String,
    pub auth: String,
}

/// POST /push/subscribe - register or replace a Push subscription for
/// the authenticated user. Returns 204. 404 when push is disabled.
pub async fn post_subscribe(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: axum::http::HeaderMap,
    Json(body): Json<SubscribeBody>,
) -> Result<Response, AppError> {
    if state.vapid.is_none() {
        return Err(AppError::NotFound);
    }
    if body.endpoint.is_empty() || body.keys.p256dh.is_empty() || body.keys.auth.is_empty() {
        return Err(AppError::BadRequest("missing subscription fields".into()));
    }
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    db::push_subscriptions::insert_or_replace(
        &state.auth,
        &user.id,
        &body.endpoint,
        &body.keys.p256dh,
        &body.keys.auth,
        user_agent.as_deref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
```

- [ ] Edit `server/src/routes/mod.rs`. Add `mod push;` next to the other
      route modules and register the three routes inside `build_router`:

```rust
.route("/sw.js", get(push::get_service_worker))
.route("/push/vapid-public-key", get(push::get_vapid_public_key))
.route("/push/subscribe", post(push::post_subscribe))
```

      Place these directly under the existing `/settings` routes for
      grouping (the Push registration flow is part of the settings UX).

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/assets/sw.js server/src/routes/push.rs server/src/routes/mod.rs`

### Task 7 - Settings: form field, setter, template checkbox

- [ ] Edit `server/src/routes/settings.rs`. Extend `SettingsForm` and the
      `post_settings` handler:

```rust
#[derive(Deserialize)]
pub struct SettingsForm {
    #[serde(default)]
    pub read_receipts_enabled: Option<String>,
    #[serde(default)]
    pub is_profile_public: Option<String>,
    #[serde(default)]
    pub notify_browser_enabled: Option<String>,
    #[serde(default)]
    pub notify_sound_enabled: Option<String>,
    #[serde(default)]
    pub notify_push_enabled: Option<String>,
}
```

      And in `post_settings`:

```rust
let browser = form.notify_browser_enabled.is_some();
let sound = form.notify_sound_enabled.is_some();
let push = form.notify_push_enabled.is_some() && state.push_available();
db::auth::set_notification_prefs(&state.auth, &user.id, browser, sound, push).await?;
```

      The `&& state.push_available()` clause defends against a user
      submitting `notify_push_enabled=1` from a manually-crafted POST
      when push is server-disabled - the column stays `0` in that case.

- [ ] Edit `server/src/views/settings.rs`. Add `pub push_available: bool`
      to `UserSettingsPage`. In `get_settings`, populate it from
      `state.push_available()`.

- [ ] Edit `server/templates/settings/page.html`. Locate the existing
      pair of checkboxes (lines around 78 and 82 by the earlier grep).
      Add a third under them:

```html
<label class="flex items-start gap-2 mt-2">
  <input type="checkbox" name="notify_push_enabled" value="1"
         {% if user.notify_push_enabled %}checked{% endif %}
         {% if !push_available %}disabled{% endif %}
         class="mt-1">
  <span>
    <span class="font-medium">Enable push notifications (works when tab is closed)</span>
    {% if !push_available %}
      <br><span class="text-xs text-slate-500">Unavailable - server is not configured for push.</span>
    {% else %}
      <br><span class="text-xs text-slate-500">A browser permission prompt will appear on the next mention.</span>
    {% endif %}
  </span>
</label>
```

      Match the surrounding markup: the existing two checkboxes use a
      similar layout. Adjust class names to be consistent.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/src/routes/settings.rs server/src/views/settings.rs server/templates/settings/page.html`

### Task 8 - Page-side service worker registration in `layout.html`

- [ ] Edit `server/templates/layout.html`. Two changes:

  1. The `lc-mention-counts` div gains a `data-push-enabled` attribute:

```html
<div id="lc-mention-counts" class="hidden"
     data-base-title="lets-chat"
     data-browser-enabled="{% if user.notify_browser_enabled %}1{% else %}0{% endif %}"
     data-sound-enabled="{% if user.notify_sound_enabled %}1{% else %}0{% endif %}"
     data-push-enabled="{% if user.notify_push_enabled %}1{% else %}0{% endif %}"></div>
```

  2. Inside the existing notification-bus IIFE, add the Push registration
     block. Place it directly after the `permPrompted` declaration and
     before `function refresh()`:

```javascript
var pushEnabled = cfg.getAttribute('data-push-enabled') === '1';
var pushTried = false;

function urlBase64ToUint8Array(s){
  var pad = '='.repeat((4 - s.length % 4) % 4);
  var b64 = (s + pad).replace(/-/g, '+').replace(/_/g, '/');
  var raw = atob(b64);
  var arr = new Uint8Array(raw.length);
  for (var i = 0; i < raw.length; i++) arr[i] = raw.charCodeAt(i);
  return arr;
}

async function tryRegisterPush(){
  if (pushTried || !pushEnabled) return;
  if (!('serviceWorker' in navigator) || !('PushManager' in window)) return;
  if (!('Notification' in window) || Notification.permission !== 'granted') return;
  pushTried = true;
  try {
    var resp = await fetch('/push/vapid-public-key');
    if (!resp.ok) return;
    var data = await resp.json();
    var reg = await navigator.serviceWorker.register('/sw.js', { scope: '/' });
    var existing = await reg.pushManager.getSubscription();
    var sub = existing || await reg.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: urlBase64ToUint8Array(data.key),
    });
    var json = sub.toJSON();
    await fetch('/push/subscribe', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        endpoint: json.endpoint,
        keys: { p256dh: json.keys.p256dh, auth: json.keys.auth },
      }),
    });
  } catch (e) { console.warn('push registration failed', e); }
}
```

      Then call `tryRegisterPush()` exactly twice inside the existing
      `fireNotification` body:

  - At the start, before the `Notification.permission === 'denied'`
    check, kick it off if permission is already granted (handles the
    second-or-later mention when the user previously granted permission
    in a prior session).
  - Inside the `Notification.requestPermission()` callback, after the
    user approves, call `tryRegisterPush()` again.

      Specifically:

```javascript
function fireNotification(d){
  if (!browserEnabled || !('Notification' in window)) return;
  if (Notification.permission === 'denied') return;
  if (Notification.permission !== 'granted') {
    if (!permPrompted) {
      permPrompted = true;
      Notification.requestPermission().then(function(p){
        if (p === 'granted') tryRegisterPush();
      });
    }
    return;
  }
  tryRegisterPush();
  // ... existing title/body/tag rendering ...
}
```

      The `tryRegisterPush()` call is idempotent via the `pushTried`
      guard, so a long-lived tab with many mentions does the work
      exactly once.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/templates/layout.html`

### Task 9 - Wire `push::dispatch` into the three Mentioned-broadcast sites

- [ ] Edit `server/src/routes/room.rs`. Three sites add a `push::dispatch`
      call directly after each `state.hub.broadcast_to_user(...)` for a
      `Mentioned` event:

  **Site 1: `post_message`, room-mention loop** (around line 330):

```rust
for t in &added {
    let event = ChatEvent::Mentioned {
        kind: "mention".into(),
        room_id: room.id,
        room_type: room.room_type.clone(),
        room_label: format!("#{}", room.name),
        message_id: new_id,
        mentioned_user_id: t.user_id.clone(),
        author_label: author_label.clone(),
        snippet: snippet.clone(),
        target_path: format!("/room/{}", room.id),
    };
    state.hub.broadcast_to_user(&t.user_id, &event);
    crate::push::dispatch(&state, &t.user_id, &event).await;
}
```

  **Site 2: `post_message`, DM branch** (around line 352):

```rust
let event = ChatEvent::Mentioned {
    kind: "dm".into(),
    room_id: room.id,
    room_type: "dm".into(),
    room_label: author_label.clone(),
    message_id: new_id,
    mentioned_user_id: peer_id.clone(),
    author_label,
    snippet,
    target_path: format!("/dm/{}", user.id),
};
state.hub.broadcast_to_user(&peer_id, &event);
// FUTURE: when the DM-mute phase lands, this bypass becomes
// conditional on dm_mute_state(user, peer).
crate::push::dispatch(&state, &peer_id, &event).await;
```

      The DM bypass marker comment lives at the dispatch call site here,
      next to the `kind: "dm"` event.

  **Site 3: `patch_message`, mention reconcile loop, `added` arm** (around
      line 563):

```rust
for t in &added {
    let event = ChatEvent::Mentioned {
        kind: "mention".into(),
        // ...rest as above...
    };
    state.hub.broadcast_to_user(&t.user_id, &event);
    crate::push::dispatch(&state, &t.user_id, &event).await;
}
```

      `MentionCleared` (the `removed` loop) does NOT trigger Push -
      Push is fire-and-forget; once delivered to a Push service the
      server cannot recall it, so emitting a clearing Push would be
      noise. The in-app `MentionCleared` event continues to fire so the
      bus decrements its counter.

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/src/routes/room.rs`

### Task 10 - Integration tests with `MockPushClient`

- [ ] Create `server/tests/push_dispatch.rs`. Setup mirrors
      `server/tests/routes_room_mute.rs`: in-memory pools loaded with
      every migration including `0009`/`0003`, two seeded users (sender
      + recipient), a public room with both as members, and an
      `AppState` constructed with `vapid: Some(test_keypair)` and
      `push_client: Arc::new(MockPushClient::default())`.

      Tests:

  1. `dispatch_sends_to_recipient_subscriptions`:
     - Recipient has `notify_push_enabled = true` and one row in
       `push_subscriptions`.
     - Build a `Mentioned { kind: "mention", room_id, ... }` event and
       call `push::dispatch(&state, &recipient.id, &event).await`.
     - `tokio::task::yield_now().await` (twice) so spawned tasks settle.
     - Assert `mock.sent.lock().await.len() == 1` and the recorded
       payload bytes parse to JSON with the expected `title`, `body`,
       `tag`, `data.target_path`.
  2. `dispatch_skips_when_notify_push_disabled`:
     - Same setup but `notify_push_enabled = false`.
     - Assert `mock.sent.lock().await.is_empty()`.
  3. `dispatch_skips_when_vapid_unconfigured`:
     - `state.vapid = None`.
     - Assert no calls.
  4. `dispatch_skips_when_room_muted_all`:
     - `notify_push_enabled = true`, but
       `set_room_mute_mode(MuteMode::All)` for the recipient on this
       room.
     - Assert no calls.
  5. `dispatch_fires_when_room_muted_except_mentions`:
     - `set_room_mute_mode(MuteMode::ExceptMentions)`. `Mentioned` kind
       == "mention".
     - Assert exactly one call.
  6. `dispatch_bypasses_room_mute_for_dm_kind`:
     - `set_room_mute_mode(MuteMode::All)` on a (synthetic) room used as
       the DM room. Build `Mentioned { kind: "dm", ... }`.
     - Assert one call (DM bypass).
  7. `dispatch_skips_when_no_subscriptions`:
     - `notify_push_enabled = true`, no rows in `push_subscriptions`.
     - Assert no calls.
  8. `dispatch_fan_out_one_per_subscription`:
     - Two subscriptions for recipient.
     - Assert two calls, distinct endpoints.
  9. `dispatch_410_deletes_subscription`:
     - Use a `MockPushClient` variant (`AlwaysGoneClient`) that returns
       `PushError::EndpointGone(endpoint)` for every send. (Add this as
       a second test-only impl in `push_dispatch.rs`.)
     - Assert `for_user(...)` returns empty after `yield_now`.
 10. `payload_dm_kind_uses_dm_title_format`:
     - Build a `Mentioned` event with `kind: "dm"`,
       `author_label: "alice"`. Run `push::payload::build`.
     - Assert the payload JSON `title == "alice (DM)"`.
 11. `payload_room_kind_uses_room_title_format`:
     - Build `kind: "mention"`, `author_label: "alice"`,
       `room_label: "#general"`. Assert
       `title == "alice in #general"`.

- [ ] Run only this test file:
      `./dev/cargo test -p lets-chat-server --test push_dispatch`

- [ ] `./dev/cargo test -p lets-chat-server` (full suite)

- [ ] `git add server/tests/push_dispatch.rs`

### Task 11 - Final verification + smoke list

- [ ] `just check-server`
- [ ] `just check-server-saas` (Push compiles in both binary modes; no
      `#[cfg]` gating).
- [ ] `just check-clippy`
- [ ] `just check-clippy-saas`
- [ ] `just check-fmt` (run `./dev/cargo fmt --all` if it complains).
- [ ] `just test`
- [ ] `just test-saas`
- [ ] `just verify`

- [ ] **Manual smoke list.** Two-browser test pairs A and B; A is the
      recipient under test, B sends mentions/DMs. Run `just dev-web-local`
      with `LETS_CHAT_SECRET_KEY=devkey` exported so Push is available.

  **Chrome desktop (A):**
  1. Log in. Confirm `/settings` shows the new "Enable push
     notifications" checkbox, enabled.
  2. Tick the box, submit, reload `/settings`, confirm checkbox
     remains checked.
  3. Open a room with B as the other user. Have B post `@a hello`.
     Confirm Chrome shows the OS-level permission prompt; accept.
  4. Have B post a second `@a` mention. Confirm a Chrome OS
     notification appears with title `"<B's name> in #<room>"`, body
     containing the snippet, and the lets-chat icon.
  5. Click the OS notification. Confirm the existing lets-chat tab
     focuses and is on the target room (or, if no tab is open,
     a new tab opens to the room).
  6. With the room A is currently looking at being room R, have B
     mention A in room R. Confirm NO OS notification fires (the SW
     visibility-suppression branch). The in-app notification surface
     still updates (title flash, mention chip).
  7. Close Chrome entirely. Have B post `@a try again`. Reopen
     Chrome. Confirm the OS notification was delivered while the
     browser was closed.

  **Firefox desktop (A):** repeat steps 1-7. Note: Firefox's autopush
     service has different latency characteristics; allow ~5s for
     delivery in step 7.

  **Edge desktop (A):** repeat steps 1-7. Edge uses Microsoft's WNS
     under FCM-compatible endpoints; behavior should match Chrome.

  **Chrome Android (A):** repeat steps 1-6. Step 7 is more strict on
     Android: confirm the notification delivers even when the browser
     is fully backgrounded (swipe Chrome out of recents, then have B
     post the mention).

  **DM test (any browser):** with A's `notify_push_enabled = true`
     and the per-room mute UI not applicable to DMs, have B send
     `/dm/<A>` "ping". Confirm the Push fires with title `"<B's name>
     (DM)"`. Open `/room/<R>` and apply `mute_mode = all` to that
     room. Have B post `@a` in R - confirm NO Push (room mute). Have
     B send another DM - confirm Push fires (DM bypass).

  **Disable test:** uncheck `notify_push_enabled` in `/settings`. Have
     B post `@a`. Confirm no Push fires (the in-app surface still
     fires because `notify_browser_enabled` stays on).

  **Cleanup test:** in DevTools (Application > Service Workers),
     unregister `/sw.js`. Wait 60s. Have B post `@a`. Confirm no Push
     fires (subscription is gone from the browser side; server may
     still attempt one send and get 410, then delete the row).
     Re-enable: a new mention triggers re-registration via
     `tryRegisterPush()`.

  **No-secret-key test:** stop the server, unset
     `LETS_CHAT_SECRET_KEY`, restart. Confirm `/settings` shows the
     Push checkbox disabled with the "Unavailable" help text.
     `GET /push/vapid-public-key` returns 404.

## Things to confirm

- **SMTP password is not currently encrypted.** The README mentions it but
  the implementation in `server/migrations/settings/0001_create_tables.sql`
  stores `smtp_pass` as plaintext in the `settings` k/v table, and there
  is no encrypt-on-write code path in `server/src/db/settings.rs`. The
  user's instruction "encrypt the VAPID private key the same way the SMTP
  password is encrypted" therefore points at a non-existent pattern;
  this plan instead matches the **TOTP** pattern (the only other
  consumer of `LETS_CHAT_SECRET_KEY`). Encrypting SMTP retroactively is
  out of scope for this phase, but the next time someone touches admin
  settings they should consider it. If the user wants SMTP encryption
  bundled into Phase 16 (one extra Task with parallel migration shape),
  flag it before execution.

- **Push gated on `LETS_CHAT_SECRET_KEY`.** Because the VAPID private
  key is encrypted-at-rest, the env var is now load-bearing for two
  features instead of one. A self-hoster who currently runs without the
  key (i.e. has 2FA disabled and has never noticed) will see Push
  silently unavailable. The settings UI surfaces this with the
  disabled-checkbox + help-text combination, but if you'd rather have
  Push work even without a key (storing the VAPID private key
  plaintext in `settings.db`), say so and I'll swap to that. Threat
  model rationale for the plaintext alternative: anyone who has read
  access to `settings.db` already has read access to message bodies,
  hashed passwords + salts, encrypted TOTP secrets, and session tokens
  in `auth.db`. The VAPID private key gives an attacker the ability to
  send Push to existing subscribers as the server, which is materially
  less harmful than the rest of the database contents. Marginal harm
  of plaintext VAPID is small; the gating-on-a-key requirement is a
  hidden activation requirement. **Default in this plan: encrypted +
  gated, matching the user's instruction.**

- **`web-push` 0.10 API surface.** The `WebPushMessageBuilder`,
  `VapidSignatureBuilder`, and `IsahcWebPushClient` names used in
  `IsahcPushClient::send` reflect the crate's published API at the time
  this plan was written. If the implementer finds the actual API has
  drifted (the crate's 0.x cadence is not glacial), match the current
  signatures - the trait wrapper isolates that churn from the rest of
  the codebase.

- **`bytes::Bytes` vs `Vec<u8>` on the trait.** The trait takes `Bytes`
  for cheap cloning across spawned tasks. If a future change wants to
  stream payloads, `Bytes` is also the right pre-positioned type.
  Confirm before substituting `Vec<u8>`.

- **`LETS_CHAT_PUSH_CONTACT` env var.** Added as the VAPID `sub` claim
  contact. Defaults to `mailto:admin@localhost`. Some Push services
  (notably Mozilla's) reject `localhost` contacts with HTTP 401. If
  smoke testing uncovers this, change the default to a real address
  per deployment, or surface it in the admin settings UI in a follow-up.

- **Sleep-based test in `db_push_subscriptions::bump_last_seen_updates_timestamp`.**
  SQLite's `datetime('now')` has second resolution, so the test sleeps
  1.1s to observe a difference. If the suite's overall runtime is
  sensitive, drop this test or replace it with a manual UPDATE that
  rewinds `last_seen_at` before the bump. Not changing now.
