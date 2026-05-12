# Plan Phase 22: Email Digest of Missed Mentions and DMs

Close the notification stack at its last meaningful gap: the offline user. After this phase, a user who closes their laptop on Friday and opens it Monday morning receives a single email summarising every mention and unread DM they missed. The digest is the catch-all behind the in-app surface (phase 14), per-room/per-DM mute (phases 15, 17), and Web Push (phase 16). It complements those transports for the case where the user simply was not online when activity happened.

## Out of scope

- Other notification channels: no SMS, no webhooks, no Slack/Discord forwarding. Email only.
- Rich email templates: no images, no branding, no embedded SVGs. Plaintext + simple HTML multipart only.
- Per-channel granularity: a single digest toggle per user. Separate "email me mentions" / "email me DMs" toggles land later if anyone asks.
- Quiet hours and per-user schedules: hardcoded hourly tick with a 1-hour quiet period.
- Push delivery tracking: see Architecture for the offline-since heuristic that replaces it.
- An end-user "preview my next digest" page: the admin SMTP page gets a "send test email" button (table-stakes for operator validation), but regular users do not get a preview surface.
- Unified notification dispatch abstraction: each transport stays inline. We now have three transports (in-app, Push, email) sharing some logic; the right time to abstract is when a fourth transport forces it.

## Background and what's already in place

Reading the code surfaces six things that shape the plan:

1. **SMTP plumbing is config-only.** `settings.db` has `smtp_host`, `smtp_port`, `smtp_user`, `smtp_pass` rows in the key-value `app_settings` table (migration `settings/0001_create_tables.sql`). The admin form at `routes/admin.rs` exposes a fifth field, `smtp_from`, that no migration ever persists: it is a dangling reference. No `lettre` (or any SMTP crate) is in `Cargo.toml`. No `send_email()` function exists. The phase has to build email-sending from scratch, including migrating the existing key-value rows into a typed `smtp_settings` table.

2. **`crate::crypto::seal` / `open` already exist** for AES-256-GCM at-rest encryption under the `LETS_CHAT_SECRET_KEY`-derived key (used by VAPID in phase 16 and TOTP). Encrypting the SMTP password reuses that helper directly. No new crypto code.

3. **`users.last_active_at`** is updated on every HTTP request via `db::auth::touch_user_activity` (called from `routes/mod.rs:188`). It is **not** bumped by the WebSocket path. `db::auth::mark_idle_users` flips users from `'active'` to `'idle'` when `last_active_at < now - 30min`; the scanner runs every 60s from `spawn_idle_scanner` in `main.rs:127`.

   This means today, a user who keeps the tab open in the background but does not interact (no posts, clicks, or navigations) has a stale `last_active_at` that does not reflect "the in-app notification surface is alive." Overloading `last_active_at` with WS bumps would change idle-flip semantics (a user in a busy room would never go idle). So this phase introduces a **separate** `users.last_ws_seen_at` column for the digest's "user's app was alive" signal, and leaves `last_active_at` (and idle-flip) untouched.

4. **Mentions and DM unread state live in two tables.** `mentions(message_id, mentioned_user_id, read_at, ...)` for room mentions; `dm_read_state(user_id, room_id, last_read_message_id)` for DM watermarks. The digest "missed" query has to read both. There is no `mention_kind` discriminator: a `@channel` in a 50-person room writes 50 rows that are indistinguishable from `@username` rows. That is fine; the digest does not need to differentiate.

5. **Per-room mute state** lives in `room_notification_settings(user_id, room_id, mute_mode)` with three modes: `'none'`, `'except_mentions'`, `'all'`. The digest reuses the existing predicate from `db::notifications::room_mute_mode`: a muted-`all` room contributes nothing, and `'except_mentions'` rooms contribute only their mention rows (which is exactly what the `mentions` table already stores). DM mute (phase 17) lives in `dm_notification_settings(user_id, peer_user_id, mute_mode)` with the same three modes; same predicate.

6. **There is already one background task** in `main.rs`: `spawn_idle_scanner` uses `tokio::spawn` + `tokio::time::interval(60s)`. The digest tick follows the same pattern with `interval(3600s)`. No new scheduling abstraction.

## Architecture

### What "missed" means

A room mention is **missed** for a user iff:

```text
mentions.read_at IS NULL
AND mentions.created_at > MAX(user.last_active_at, COALESCE(user.last_ws_seen_at, ''))
AND mentions.created_at > now - 7 days
```

That is: (a) the user did not advance their read watermark past it, (b) neither their HTTP activity nor their WS connection was alive when the mention arrived, and (c) it is not older than the 7-day mailbomb cap. The combined-MAX predicate handles three real cases:

- User actively chatting on web: `last_active_at` is recent, mention is suppressed.
- User has tab open but is AFK: `last_ws_seen_at` is recent (the WS pushed the `Mentioned` frame to them, so the in-app surface fired), mention is suppressed.
- User closed all tabs and walked away: both columns are stale; mention is digest-eligible.

For DMs, the equivalent predicate is:

```text
m.id > COALESCE(dm_read_state.last_read_message_id, 0)
AND m.author_id != recipient_id
AND m.created_at > MAX(user.last_active_at, COALESCE(user.last_ws_seen_at, ''))
AND m.created_at > now - 7 days
```

No new "delivery tracking" table. The combined timestamp covers what we would otherwise have learned from per-mention Push outcomes. Documented YAGNI: revisit if "I got an email about a mention I'd already seen on my phone" turns out to be a frequent complaint.

### When the digest fires

A user is digest-eligible at tick time iff:

```text
unread_mentions OR unread_dms exist for the user (within 7 days)
AND user.notify_email_digest_enabled = 1
AND user.email IS NOT NULL AND user.email <> ''
AND state.email_client IS NOT None
AND MAX(last_active_at, COALESCE(last_ws_seen_at, '')) < now - 1h
AND (last_digest_sent_at IS NULL
     OR last_digest_sent_at < MAX(last_active_at, COALESCE(last_ws_seen_at, '')))
```

The last predicate gives **one digest per offline session, no matter how long the session lasts**. Once the user comes back online and bumps either timestamp, `last_digest_sent_at < MAX(...)` becomes true, and the next time they go offline they are eligible for one more email. A user who never comes back gets exactly one email; a user who closes their laptop every evening gets one email per evening. No fixed-window counter, no "are we in the same window" arithmetic.

Tick cadence: hourly. `spawn_digest_sender(state)` in `main.rs`, modeled on `spawn_idle_scanner`. `tokio::time::interval(3600s)`, loop body queries for eligible users and sends. Cancellation is implicit on runtime shutdown; no graceful drain.

### What goes in the digest

- **Time window** = `created_at > MAX(last_active_at, last_ws_seen_at)`, capped at 7 days.
- **Max items** = 50 across all sections; after that, a "... and N more, open lets-chat to see them" footer.
- **Sections**: DMs first (higher signal), then rooms.
- **Within DMs**: one section per peer, oldest-first. Format per item: "{peer} at {timestamp}: {snippet}" with a deep link.
- **Within rooms**: one section per room, oldest-first. Format per item: "{author} at {timestamp}: {snippet}" with a deep link.
- **Snippet** = first 140 chars of the message body, word-boundary truncated, plaintext-escaped, with `@username` mentions bolded (HTML part) or left literal (plaintext part). No code-block rendering, no custom-emoji rendering, no embedded images. URLs linkified in the HTML part, left literal in plaintext.
- **Subject**: `[lets-chat] N new mentions and M direct messages`, with the zero-clause dropped when one side is empty.
- **Deep links**: room mentions link to `<LETS_CHAT_SERVER_URL>/room/<room_id>#m<message_id>` (existing anchor convention from phase 7). DM messages link to `<LETS_CHAT_SERVER_URL>/dm/<peer_id>#m<message_id>`.
- **Multipart**: `multipart/alternative` with both `text/plain` and `text/html` parts containing logically identical content.

### Mute interaction

Muted rooms contribute zero rows to the digest. In the "find missed mentions" query, the left-join against `room_notification_settings` excludes `mute_mode = 'all'`. For `mute_mode = 'except_mentions'`, the user still receives mentions (the `mentions` table already only contains mention rows; ambient room messages are not in it). DM mute (`mute_mode = 'all'` in `dm_notification_settings`) excludes that DM thread's unread DMs.

### User setting and opt-in default

New column on `users`: `notify_email_digest_enabled INTEGER NOT NULL DEFAULT 0`. Opt-in (default off), matching the phase 16 Push precedent. The settings page at `/settings` gets a fourth checkbox under "Notifications":

```html
<input type="checkbox" name="notify_email_digest_enabled" value="1"
       {% if user.notify_email_digest_enabled %}checked{% endif %}
       {% if !email_available %}disabled{% endif %}>
```

When `email_available` is false, the label reads "(unavailable: this server is not configured for email)." `email_available: bool` is a new field on `UserSettingsPage`, populated from `state.email_client.is_some()`.

**Admin-configurable default for new users.** A new key in the existing `app_settings` table, `default_notify_email_digest`, controls whether newly-registered users start with the digest enabled. Default is `'0'` (opt-in for the instance). Operators who want opt-out semantics flip this to `'1'`. Existing users are not retroactively changed when the admin flips the toggle. The user-registration path (`db::auth::insert_user`) reads this default at insert time.

### Scheduler shape

```rust
fn spawn_digest_sender(state: AppState) {
    const TICK_SECS: u64 = 3600;
    const QUIET_PERIOD_SECS: i64 = 3600;
    const MAX_ITEMS: usize = 50;
    const TIME_WINDOW_DAYS: i64 = 7;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        tick.tick().await;            // skip the immediate fire
        loop {
            tick.tick().await;
            if let Err(e) = email::digest::run_tick(
                &state, QUIET_PERIOD_SECS, MAX_ITEMS, TIME_WINDOW_DAYS
            ).await {
                tracing::warn!(error = %e, "digest tick failed");
            }
        }
    });
}
```

Spawned unconditionally. `run_tick` short-circuits at the top if `state.email_client` is `None`. Cancellation happens implicitly when the runtime shuts down.

### `EmailClient` trait

Mirror of phase 16's `PushClient`. One method, two implementations:

```rust
#[async_trait::async_trait]
pub trait EmailClient: Send + Sync {
    async fn send(&self, msg: EmailMessage) -> Result<(), EmailError>;
}

pub struct EmailMessage {
    pub to: String,
    pub from: String,
    pub subject: String,
    pub text_body: String,
    pub html_body: String,
}
```

- `LettreEmailClient` (prod). Holds the loaded `SmtpConfig`. Each `send()` constructs a fresh `SmtpTransport`, sends, drops. Rationale: digest sends are infrequent (hourly tick, typically a handful per call), the STARTTLS handshake cost is negligible relative to the actual send, and per-send construction sidesteps "transport disconnected over an idle period" complexity.
- `MockEmailClient` (test-only). `Mutex<Vec<EmailMessage>>`, records every call. Injected in tests in place of `LettreEmailClient`.

No `send_batch`, no template rendering inside the trait. The trait has exactly one method.

### WS-receive activity tracking

New `users.last_ws_seen_at TEXT` column (nullable, default NULL). Bumped from the WS handler at exactly two moments:

- **On WS connection-open.** One write per new connection.
- **On outbound `Mentioned` frame send to that user.** Throttled per-connection: each connection holds a `last_bump: tokio::time::Instant` and skips the DB write if less than 5 minutes have elapsed since the last bump.

The throttle is per-connection, not per-user: a user with two tabs writes at most two updates per 5min. SQLite handles this fine; the write is a single-row UPDATE on the primary key.

**Why bump on outbound `Mentioned` specifically, not every outbound frame.** The semantic we want is "the in-app notification surface had a chance to fire for this user." That happens when a `Mentioned` event reaches them. Bumping on every outbound frame would also be defensible (broader signal of "tab is alive"), but it adds writes for ambient `NewMessage` traffic that does not interact with the digest semantics. Outbound-`Mentioned`-only is the precise signal.

Idle-flip semantics in `mark_idle_users` are **unchanged**. The scanner still uses `last_active_at` only. A comment at the scanner's call site documents the split:

```rust
// Idle status reflects HTTP-request activity only (`last_active_at`).
// The separate `last_ws_seen_at` column is the digest's "in-app surface
// was alive" signal; it deliberately does NOT participate in idle-flip,
// so a user with a tab open in a busy room continues to flip to idle
// after 30min of no HTTP interaction.
```

### SMTP config: schema rework

Today: key-value rows in `app_settings` (plaintext password). After this phase: a typed singleton table.

```sql
CREATE TABLE IF NOT EXISTS smtp_settings (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    host                TEXT,
    port                INTEGER,
    username            TEXT,
    password_encrypted  BLOB,
    password_nonce      BLOB,
    from_address        TEXT,
    tls_mode            TEXT NOT NULL DEFAULT 'starttls',
    updated_at          TEXT NOT NULL DEFAULT (datetime('now'))
);
```

The migration **discards** the existing plaintext SMTP password. Operators re-enter it after upgrade. This is a one-shot inconvenience documented in the release notes; the alternative (silently leaving plaintext rows lying around) is worse for security posture and would require a second migration later to actually clean up. The `from_address` column is new (it was only ever referenced by the form, never persisted).

### Failure modes

- **`LETS_CHAT_SECRET_KEY` unset.** SMTP password cannot be decrypted (same gate as VAPID). `state.email_client = None`. Digest tick logs `debug!("email digest skipped: secret key not configured")` and returns. Settings checkbox renders disabled. Admin SMTP page shows a banner: "Email-sending is disabled because LETS_CHAT_SECRET_KEY is not set."
- **SMTP host empty or row absent.** `state.email_client = None`. Same handling as above; banner reads "SMTP is not configured."
- **SMTP transport error during a send.** Log warn with the full error. Do **not** call `set_last_digest_sent_at` for the failed user, so they remain eligible next tick. No per-user retry within the same tick. If SMTP is broken for everyone, every tick will log loudly and the operator will see it.

### Admin SMTP page changes

The existing form gains:

- A `from_address` field (currently a dangling reference; now actually persisted).
- A `tls_mode` selector: `starttls` (default) / `tls` / `none`.
- A "Send test email" button. POST `/admin/settings/smtp/test`, sends a hardcoded one-line email to the admin user's own address. Surfaces success or the full SMTP error in a banner.
- A "Default digest for new users" toggle (the admin-configurable default described above).
- The password input is write-only: the form renders an empty `<input type="password">`. Submit-blank means "leave the existing encrypted value alone." A separate "Clear password" button is the only way to remove it without entering a new one.

## Tech Stack

New crates:

```toml
lettre = { version = "0.11", default-features = false, features = ["smtp-transport", "builder", "tokio1-rustls-tls", "rustls-tls"] }
```

Lettre 0.11 with rustls (not native-tls) keeps the binary buildable without OpenSSL on the slim Docker image. `builder` brings the `MessageBuilder` and `multipart/alternative` helpers.

Already-direct deps reused: `aes-gcm` (encryption), `async-trait` (trait), `serde_json` (form payloads), `regex` (snippet mention-bolding via the existing token pattern).

No build steps change. Tailwind, Bun, just recipes are untouched.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Add | `server/migrations/auth/0010_digest_columns.sql` | Adds `last_ws_seen_at`, `notify_email_digest_enabled`, `last_digest_sent_at` to `users`; adds a partial index for digest eligibility. |
| Add | `server/migrations/settings/0004_smtp_settings.sql` | Typed `smtp_settings` table; migrates from `app_settings` key-value rows; drops the old plaintext password and the no-longer-needed key-value rows. |
| Add | `server/migrations/settings/0005_default_email_digest.sql` | Seeds `default_notify_email_digest = '0'` into `app_settings`. |
| Edit | `server/Cargo.toml` | Add lettre. |
| Edit | `server/src/models/user.rs` | Add `notify_email_digest_enabled: bool`, `last_ws_seen_at: Option<String>`, `last_digest_sent_at: Option<String>` to `UserRecord` and `User`. |
| Edit | `server/src/db/auth.rs` | Extend every `UserRecord` SELECT to include the three new columns; add `bump_last_ws_seen`, `set_last_digest_sent_at`; extend `set_notification_prefs` with a fourth `email_digest` arg; thread `default_notify_email_digest` into `insert_user`. |
| Add | `server/src/db/smtp_settings.rs` | `SmtpConfig` row type + `TlsMode` enum + `load`, `save`, `clear_password`. Encryption via `crate::crypto`. |
| Edit | `server/src/db/settings.rs` | Remove the four SMTP key-value getters (moved to `smtp_settings`). |
| Edit | `server/src/db/mod.rs` | `pub mod smtp_settings;`. |
| Add | `server/src/email/mod.rs` | `EmailClient` trait + `EmailMessage` + `EmailError` + `LettreEmailClient` + `MockEmailClient`. |
| Add | `server/src/email/digest.rs` | `run_tick` + eligibility query + per-user "build and send digest" + the `Digest` view model. |
| Add | `server/src/views/email_digest.rs` | Askama template structs for the HTML and plaintext bodies. |
| Add | `server/templates/email_digest.html` | HTML email body. |
| Add | `server/templates/email_digest.txt` | Plaintext email body. |
| Edit | `server/src/lib.rs` | `pub mod email;`. |
| Edit | `server/src/state.rs` | Add `email_client: Option<Arc<dyn EmailClient>>` to `AppState`. |
| Edit | `server/src/main.rs` | Construct `email_client` at startup; spawn `spawn_digest_sender(state.clone())`. |
| Edit | `server/src/routes/ws.rs` | Bump `last_ws_seen_at` on connection-open and on outbound `Mentioned` frame send, throttled per-connection. |
| Edit | `server/src/routes/settings.rs` | New form field; pass `email_digest` to `set_notification_prefs`. |
| Edit | `server/src/views/settings.rs` | `email_available: bool` on `UserSettingsPage`. |
| Edit | `server/templates/settings/page.html` | Fourth notification checkbox. |
| Edit | `server/src/routes/admin.rs` | New SMTP form fields (`from_address`, `tls_mode`); password-write-only handling; POST `/admin/settings/smtp/test`; POST `/admin/settings/email-digest-default`. |
| Edit | `server/templates/admin/settings.html` | New form fields, "Send test email" button, status banners, admin-default-digest toggle. |
| Add | `server/tests/db_smtp_settings.rs` | Round-trip the encrypted password; `password: None` preserves existing. |
| Add | `server/tests/db_email_digest.rs` | Eligibility query for various user states; `last_digest_sent_at` gating. |
| Add | `server/tests/email_digest_dispatch.rs` | End-to-end: seed activity, run tick, assert `MockEmailClient` received the expected message. |
| Add | `server/tests/db_email_digest_default.rs` | Admin-toggled default propagates to new users; existing users unchanged. |
| Add | `server/tests/routes_ws_bump.rs` | Connection-open bumps `last_ws_seen_at`; second outbound `Mentioned` within 5min does not re-bump. |
| Add | `server/tests/routes_admin_smtp_test.rs` | "Send test email" button hits the mock client. |
| Edit | every `tests/*.rs` that opens auth or settings pools | Add the new migrations to the include list. |
| Edit | `README.md` | Document the digest cadence, quiet period, 7-day cap, opt-in default, admin-configurable default, and the SMTP password re-entry requirement after upgrade. |

## Tasks

### Task 1: WS-bump precursor (schema + signal, no digest logic yet)

Lands the `last_ws_seen_at` column and the WS-bump call sites before any digest logic depends on the signal. Isolated from the rest of the phase; easy to revert if anything goes sideways.

- [ ] Confirm next migration numbers: `ls server/migrations/auth/` (next is `0010`), `ls server/migrations/settings/` (next is `0004` then `0005`).
- [ ] `git checkout -b feat/email-digest`
- [ ] Create `server/migrations/auth/0010_digest_columns.sql`:

```sql
ALTER TABLE users ADD COLUMN last_ws_seen_at TEXT;
ALTER TABLE users ADD COLUMN notify_email_digest_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN last_digest_sent_at TEXT;

CREATE INDEX IF NOT EXISTS idx_users_digest_eligible
    ON users (notify_email_digest_enabled, last_active_at)
    WHERE notify_email_digest_enabled = 1;
```

  The partial index narrows the eligibility scan to opted-in users; on a 1000-user instance where 50 opted in, the scan is 50 rows instead of 1000.

- [ ] Add the three fields to `UserRecord` and `User` in `server/src/models/user.rs`.
- [ ] Extend every `UserRecord` SELECT in `server/src/db/auth.rs` (find-by-id, find-by-username, the `User`-mapping site at ~line 88, and the bulk loaders) to include the three new columns.
- [ ] Add to `server/src/db/auth.rs`:

```rust
pub async fn bump_last_ws_seen(pool: &SqlitePool, user_id: &str) {
    let res = sqlx::query("UPDATE users SET last_ws_seen_at = datetime('now') WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, user_id = %user_id, "bump_last_ws_seen failed");
    }
}

pub async fn set_last_digest_sent_at(pool: &SqlitePool, user_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET last_digest_sent_at = datetime('now') WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}
```

  `bump_last_ws_seen` returns `()` and logs internally because the WS hot path should never propagate a digest-related DB error. `set_last_digest_sent_at` returns `Result` because the digest tick wants to know if it failed (in which case it should not consider the user as digested).

- [ ] Edit `server/src/routes/ws.rs`. The WS connection state already holds per-connection bookkeeping (subscriptions, send sink). Add one field:

```rust
struct WsConn {
    // existing fields...
    last_ws_bump: tokio::time::Instant,
}
```

  Initialise to `Instant::now()` at connection accept, **and** call `db::auth::bump_last_ws_seen(...)` once unconditionally at accept time (the connection-open bump).

  At the existing outbound-`Mentioned` send site, before pushing the frame to the sink, check the throttle:

```rust
const WS_BUMP_THROTTLE: std::time::Duration = std::time::Duration::from_secs(300);
if conn.last_ws_bump.elapsed() >= WS_BUMP_THROTTLE {
    db::auth::bump_last_ws_seen(&state.auth, &conn.user_id).await;
    conn.last_ws_bump = tokio::time::Instant::now();
}
```

  Confirm during implementation that there is exactly **one** outbound-frame call site for `Mentioned`. If there are multiple, factor the throttle into a `WsConn::on_mentioned_send(&self, state)` helper so the throttle logic does not duplicate.

- [ ] At the `mark_idle_users` call site in `main.rs`, add the comment from the Architecture section documenting that idle-flip is HTTP-only by design.
- [ ] Add `server/tests/routes_ws_bump.rs`:
  - Spawn a WS connection; assert `users.last_ws_seen_at` is non-NULL after accept.
  - Snapshot `last_ws_seen_at`; trigger an outbound `Mentioned` for the user; assert `last_ws_seen_at` advanced.
  - Snapshot again; trigger a second outbound `Mentioned` within 5min; assert `last_ws_seen_at` did NOT advance.
- [ ] Update every existing `tests/*.rs` that opens the auth pool to include `auth/0010_digest_columns.sql` in its migration include list.
- [ ] Run `just check` and `just test`. Existing tests should pass unchanged; the new column is additive and the WS-bump is a side effect that no existing test asserts on.

### Task 2: SMTP infrastructure (typed table, encryption, lettre, "send test email")

Lands the email-sending primitive in isolation from the digest logic. After this task, an admin can send a test email manually, but no digest is yet wired.

- [ ] Add lettre to `server/Cargo.toml`:

```toml
lettre = { version = "0.11", default-features = false, features = ["smtp-transport", "builder", "tokio1-rustls-tls", "rustls-tls"] }
```

  Run `./dev/cargo build -p lets-chat-server` to confirm the feature flags resolve cleanly without OpenSSL. Lettre's feature-flag names have drifted across minor versions; if `tokio1-rustls-tls` does not resolve, fall back to the canonical name listed by `./dev/cargo metadata --filter-platform x86_64-unknown-linux-gnu | grep -A2 'lettre'` and adjust the feature list.

- [ ] Create `server/migrations/settings/0004_smtp_settings.sql`:

```sql
CREATE TABLE IF NOT EXISTS smtp_settings (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    host                TEXT,
    port                INTEGER,
    username            TEXT,
    password_encrypted  BLOB,
    password_nonce      BLOB,
    from_address        TEXT,
    tls_mode            TEXT NOT NULL DEFAULT 'starttls',
    updated_at          TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO smtp_settings (id, host, port, username, from_address, tls_mode)
    SELECT 1,
           (SELECT value FROM app_settings WHERE key = 'smtp_host'),
           CAST((SELECT value FROM app_settings WHERE key = 'smtp_port') AS INTEGER),
           (SELECT value FROM app_settings WHERE key = 'smtp_user'),
           NULL,
           'starttls'
    WHERE NOT EXISTS (SELECT 1 FROM smtp_settings WHERE id = 1);

DELETE FROM app_settings
 WHERE key IN ('smtp_host', 'smtp_port', 'smtp_user', 'smtp_pass');
```

  Note the migration **discards** the existing plaintext SMTP password; operators must re-enter it after upgrade. This is intentional (cleaning up the pre-encryption state) and documented in the release notes.

- [ ] Create `server/migrations/settings/0005_default_email_digest.sql`:

```sql
INSERT OR IGNORE INTO app_settings (key, value) VALUES ('default_notify_email_digest', '0');
```

- [ ] Create `server/src/db/smtp_settings.rs` with `SmtpConfig`, `TlsMode`, `SmtpConfigInput`, `load`, `save`, `clear_password`:

```rust
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,       // decrypted in memory
    pub from_address: String,
    pub tls_mode: TlsMode,
}

pub enum TlsMode { StartTls, Tls, None }

pub struct SmtpConfigInput {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,       // None means "leave existing alone"
    pub from_address: String,
    pub tls_mode: TlsMode,
}

pub async fn load(pool: &SqlitePool, secret_key: &[u8; 32]) -> Result<Option<SmtpConfig>, AppError>;
pub async fn save(pool: &SqlitePool, secret_key: &[u8; 32], cfg: &SmtpConfigInput) -> Result<(), AppError>;
pub async fn clear_password(pool: &SqlitePool) -> Result<(), AppError>;
```

  `load` returns `None` if `host` is empty (config not yet entered). Decryption failure returns `Err` (the operator should see this in the admin banner). `save` with `password: None` leaves the existing `password_encrypted`/`password_nonce` columns untouched; with `password: Some("")` clears them; with `password: Some(x)` re-encrypts.

  Encryption: reuse `crate::crypto::seal(secret_key, plaintext.as_bytes()) -> (nonce, ciphertext)` directly. Same shape as `db::vapid` and `db::two_factor`.

- [ ] Remove the four SMTP getters from `server/src/db/settings.rs`. The compiler will flag the admin form's references; fix them in the next step.

- [ ] Create `server/src/email/mod.rs`:

```rust
#[async_trait::async_trait]
pub trait EmailClient: Send + Sync {
    async fn send(&self, msg: EmailMessage) -> Result<(), EmailError>;
}

pub struct EmailMessage {
    pub to: String,
    pub from: String,
    pub subject: String,
    pub text_body: String,
    pub html_body: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error("smtp transport error: {0}")]
    Transport(String),
    #[error("invalid address: {0}")]
    InvalidAddress(String),
}

pub struct LettreEmailClient {
    pub config: SmtpConfig,
}

#[async_trait::async_trait]
impl EmailClient for LettreEmailClient {
    async fn send(&self, msg: EmailMessage) -> Result<(), EmailError> {
        use lettre::{Message, Transport, SmtpTransport};
        use lettre::message::{MultiPart, SinglePart, header::ContentType};
        let from = msg.from.parse().map_err(|e| EmailError::InvalidAddress(format!("{e}")))?;
        let to   = msg.to.parse().map_err(|e| EmailError::InvalidAddress(format!("{e}")))?;
        let email = Message::builder()
            .from(from)
            .to(to)
            .subject(&msg.subject)
            .multipart(MultiPart::alternative()
                .singlepart(SinglePart::builder()
                    .header(ContentType::TEXT_PLAIN).body(msg.text_body))
                .singlepart(SinglePart::builder()
                    .header(ContentType::TEXT_HTML).body(msg.html_body)))
            .map_err(|e| EmailError::Transport(format!("{e}")))?;
        let transport = match self.config.tls_mode {
            TlsMode::StartTls => SmtpTransport::starttls_relay(&self.config.host),
            TlsMode::Tls      => SmtpTransport::relay(&self.config.host),
            TlsMode::None     => SmtpTransport::builder_dangerous(&self.config.host),
        }
        .map_err(|e| EmailError::Transport(format!("{e}")))?
        .port(self.config.port);
        let transport = if let (Some(user), Some(pass)) = (&self.config.username, &self.config.password) {
            transport.credentials(lettre::transport::smtp::authentication::Credentials::new(
                user.clone(), pass.clone()))
        } else {
            transport
        }.build();
        transport.send(&email).map_err(|e| EmailError::Transport(format!("{e}")))?;
        Ok(())
    }
}

#[cfg(any(test, feature = "test-helpers"))]
pub struct MockEmailClient {
    pub sent: tokio::sync::Mutex<Vec<EmailMessage>>,
}
```

  The sketch uses Lettre's sync `SmtpTransport`. The connection is built per-send (justified above). If Lettre's async `AsyncSmtpTransport` proves easier to integrate with the tokio runtime, swap; the trait does not care.

- [ ] Update `server/src/state.rs`:

```rust
pub email_client: Option<Arc<dyn email::EmailClient>>,
```

- [ ] Update `server/src/main.rs` startup. Construct `email_client` after the settings pool is open:

```rust
let email_client: Option<Arc<dyn email::EmailClient>> = match (
    secret_key_bytes.as_ref(),
    db::smtp_settings::load(&settings_pool, secret_key_bytes.as_ref().unwrap_or(&[0u8; 32])).await,
) {
    (Some(key), Ok(Some(cfg))) if !cfg.host.is_empty() => {
        Some(Arc::new(email::LettreEmailClient { config: cfg }))
    }
    (Some(_), Err(e)) => {
        tracing::warn!(error = %e, "smtp settings present but failed to decrypt; email disabled");
        None
    }
    _ => None,
};
```

  (Refine the gating in implementation to match the existing `secret_key_bytes` shape from phase 16.)

- [ ] Edit `server/src/routes/admin.rs` and `server/templates/admin/settings.html`:
  - Replace the four removed key-value getters with `db::smtp_settings::load(...)`.
  - Add `from_address` and `tls_mode` form fields. `from_address` is required for save; the form rejects with a banner if blank.
  - Password input renders empty regardless of stored state. On submit, blank means "leave existing alone." A separate "Clear stored password" checkbox sets `password: Some("")` which the save path interprets as "clear."
  - New POST `/admin/settings/smtp/test`: constructs a hardcoded `EmailMessage`:
    - `to`: the calling admin's `users.email` (require non-empty; otherwise render an error banner).
    - `from`: `cfg.from_address`.
    - `subject`: "lets-chat SMTP test".
    - `text_body`: one line of plaintext.
    - `html_body`: same content in `<p>`.
    Calls `state.email_client.send(...)`. Surfaces success or the full `EmailError::Transport` text in a banner via a session-flash mechanism (or a query-string `?test_result=...` if no flash exists yet).
  - New POST `/admin/settings/email-digest-default`: flips the `default_notify_email_digest` row.
  - Top-of-page banners: "Email-sending is disabled because LETS_CHAT_SECRET_KEY is not set." (when secret key missing) or "Email-sending is disabled because SMTP is not configured." (when secret key present but `host` empty).

- [ ] Add `server/tests/db_smtp_settings.rs`:
  - Save with a password; load; assert plaintext round-trips.
  - Save with `password: None`; load; assert previous password is preserved.
  - Save with `password: Some("")`; load; assert password cleared (None).
- [ ] Add `server/tests/routes_admin_smtp_test.rs`: admin POSTs to `/admin/settings/smtp/test` with `state.email_client` set to a `MockEmailClient`; assert exactly one recorded send to the admin's email.
- [ ] Run `just check` and `just test`. SMTP infrastructure is now wired end-to-end.

### Task 3: Digest content (view model, templates, snippet helper)

- [ ] Create `server/src/views/email_digest.rs`:

```rust
#[derive(Template)]
#[template(path = "email_digest.html")]
pub struct DigestHtml<'a> {
    pub server_url: &'a str,
    pub dm_sections: &'a [DigestDmSection],
    pub room_sections: &'a [DigestRoomSection],
    pub overflow_count: usize,
}

#[derive(Template)]
#[template(path = "email_digest.txt")]
pub struct DigestText<'a> {
    pub server_url: &'a str,
    pub dm_sections: &'a [DigestDmSection],
    pub room_sections: &'a [DigestRoomSection],
    pub overflow_count: usize,
}

pub struct DigestDmSection { pub peer_username: String, pub peer_id: String, pub items: Vec<DigestItem> }
pub struct DigestRoomSection { pub room_name: String, pub room_id: i64, pub items: Vec<DigestItem> }
pub struct DigestItem {
    pub message_id: i64,
    pub author: String,
    pub created_at: String,            // pre-formatted "Mon 14:23"
    pub snippet_plain: String,         // for the .txt template
    pub snippet_html: String,          // for the .html template, already escaped + mention-bolded
    pub deep_link: String,
}
```

- [ ] Create `server/templates/email_digest.html`. Minimal: a `<table>` per section, no `<style>` block, no CSS classes (some clients strip them), no images. Snippet HTML is pre-rendered (mentions wrapped in `<strong>`, URLs in `<a>`); the template inserts it via `{{ item.snippet_html|safe }}`. Sections separated by `<hr>`. Each item is a row: `{{ item.author }} at {{ item.created_at }}` + a `<a href="{{ item.deep_link }}">message</a>` + `<blockquote>{{ item.snippet_html|safe }}</blockquote>`.

- [ ] Create `server/templates/email_digest.txt`. Plaintext mirror. Sections separated by a blank line and `===`. Each item: `  {{ item.author }} at {{ item.created_at }}: {{ item.snippet_plain }}` on one line, followed by `  Link: {{ item.deep_link }}` on the next.

- [ ] Add `email::digest::build_snippet(body: &str) -> (String, String)` returning `(plain, html)`:
  - Plain: trim, word-truncate to 140 chars, append `...` if truncated. No HTML escaping.
  - HTML: trim, word-truncate, HTML-escape, wrap `@\w+` substrings in `<strong>`, linkify bare URLs. Reuse the existing escape/linkify helper from `views::room::body_html` if it can be extracted cheaply; otherwise inline a minimal version (HTML escape, regex-wrap `@\w+`, regex-wrap `https?://\S+` in `<a>`). The digest's snippet does not need to match the in-app message rendering exactly; "looks like a chat snippet" is enough.

- [ ] Unit-test `build_snippet`:
  - Short string: unchanged in both forms.
  - Long string: truncated at word boundary, `...` appended.
  - HTML-special characters in body: escaped in the HTML form, untouched in the plain form.
  - `@alice` in body: wrapped in `<strong>` in HTML, literal `@alice` in plain.
  - URL in body: wrapped in `<a>` in HTML, literal in plain.

### Task 4: Eligibility query and digest tick

- [ ] Create `server/src/email/digest.rs` with `pub async fn run_tick(state: &AppState, quiet_period_secs: i64, max_items: usize, time_window_days: i64) -> Result<(), AppError>`.

- [ ] `run_tick` step 1: short-circuit. If `state.email_client.is_none()`, log debug and return Ok.

- [ ] Step 2: eligibility query against the auth pool. SQLite does not have `GREATEST`; emulate with `MAX(a, COALESCE(b, ''))` (string comparison works because timestamps are ISO 8601 lexicographically ordered):

```sql
SELECT id, username, email, last_active_at, last_ws_seen_at, last_digest_sent_at
  FROM users
 WHERE notify_email_digest_enabled = 1
   AND email IS NOT NULL AND email <> ''
   AND MAX(last_active_at, COALESCE(last_ws_seen_at, '')) < datetime('now', ?)   -- '-3600 seconds'
   AND (last_digest_sent_at IS NULL
        OR last_digest_sent_at < MAX(last_active_at, COALESCE(last_ws_seen_at, '')));
```

  Bind the quiet-period as a Rust-formatted `'-{quiet_period_secs} seconds'` string.

- [ ] Step 3, per candidate user, against the chat pool: load unread mentions in the window:

```sql
SELECT m.id AS mention_id,
       msg.id AS message_id, msg.body, msg.created_at,
       r.id  AS room_id, r.name AS room_name,
       u.username AS author_name
  FROM mentions m
  JOIN messages msg ON msg.id = m.message_id
  JOIN rooms    r   ON r.id   = m.room_id
  -- author lookup goes against the auth pool; do it as a post-pass
  LEFT JOIN room_notification_settings rns
         ON rns.user_id = m.mentioned_user_id AND rns.room_id = m.room_id
 WHERE m.mentioned_user_id = ?
   AND m.read_at IS NULL
   AND msg.created_at > ?                      -- bound = MAX(last_active_at, last_ws_seen_at)
   AND msg.created_at > datetime('now', ?)     -- '-7 days'
   AND (rns.mute_mode IS NULL OR rns.mute_mode <> 'all')
 ORDER BY r.id, msg.created_at;
```

  Author usernames are looked up by a single bulk call to `db::auth::display_names_for_ids` over the unique author-id set (phase 19 helper, used elsewhere for the same auth-pool/chat-pool join).

- [ ] Step 4, per candidate user, against the chat pool: load unread DMs in the window:

```sql
SELECT msg.id AS message_id, msg.body, msg.created_at, msg.author_id,
       r.id   AS room_id,
       peer.id AS peer_id    -- two-row room_members; pick the non-self side
  FROM messages msg
  JOIN rooms r ON r.id = msg.room_id AND r.room_type = 'dm'
  JOIN room_members rm_self ON rm_self.room_id = r.id AND rm_self.user_id = ?
  JOIN room_members peer   ON peer.room_id   = r.id AND peer.user_id   <> ?
  LEFT JOIN dm_read_state s
         ON s.user_id = ? AND s.room_id = r.id
  LEFT JOIN dm_notification_settings dns
         ON dns.user_id = ? AND dns.peer_user_id = peer.user_id
 WHERE msg.author_id <> ?
   AND msg.id > COALESCE(s.last_read_message_id, 0)
   AND msg.created_at > ?
   AND msg.created_at > datetime('now', ?)
   AND (dns.mute_mode IS NULL OR dns.mute_mode <> 'all')
 ORDER BY peer.user_id, msg.created_at;
```

  Peer usernames are bulk-looked up in the same auth-pool pass as the mention authors.

- [ ] Step 5: assemble the digest. If total items is 0, skip this user (everything turned out to be muted or read). Truncate to `max_items`, set `overflow_count = total - max_items`. Build `DigestDmSection` per peer, `DigestRoomSection` per room, in the order returned (sorted by created_at within each). Construct `DigestHtml` and `DigestText` view models. Render both to strings via Askama.

- [ ] Step 6: subject line:

```rust
let subject = match (mention_count, dm_count) {
    (0, m) => format!("[lets-chat] {m} new direct messages"),
    (n, 0) => format!("[lets-chat] {n} new mentions"),
    (n, m) => format!("[lets-chat] {n} new mentions and {m} direct messages"),
};
```

  (Singularise "1 mention" / "1 direct message" via a tiny helper.)

- [ ] Step 7: send. Build `EmailMessage { to: user.email, from: from_address, subject, text_body, html_body }`, call `state.email_client.as_ref().unwrap().send(msg).await`. On Ok, call `db::auth::set_last_digest_sent_at(user_id)`. On Err, log warn with the error and the user_id; do NOT mark digest-sent.

- [ ] Edit `server/src/main.rs` to add `spawn_digest_sender(state.clone())` alongside `spawn_idle_scanner(state.clone())`. Body matches the architecture sketch.

- [ ] Add `server/tests/db_email_digest.rs`:
  - Seed five users in different states: (a) active just now, (b) active 30min ago, (c) inactive 2h, (d) inactive 2h with `last_ws_seen_at` 5min ago, (e) inactive 2h with a `last_digest_sent_at` newer than `last_active_at`.
  - Run the eligibility query.
  - Assert only user (c) is returned.
  - Then bump (c)'s `last_active_at` to now; assert (c) is no longer returned; the predicate self-resets.

- [ ] Add `server/tests/email_digest_dispatch.rs`:
  - Seed user A (`notify_email_digest_enabled = 1`, `email = "a@example.com"`, `last_active_at = '2026-05-10 12:00:00'`, no WS seen).
  - Seed three unread mentions in room X (created_at over the past day, after A's last_active_at).
  - Seed two unread DM messages from user B (same temporal pattern).
  - Inject `MockEmailClient` via a test-only constructor.
  - Run `email::digest::run_tick(&state, 3600, 50, 7)`.
  - Assert exactly one recorded send, addressed to `a@example.com`.
  - Assert subject is `[lets-chat] 3 new mentions and 2 direct messages`.
  - Assert text body contains the snippet for each of the 5 items in the right order (DMs first, then room).
  - Assert HTML body contains a `<strong>` wrapper for `@a` mentions in snippets.
  - Assert `users.last_digest_sent_at` is now non-NULL for user A.
  - Run a second tick within seconds; assert NO new send (the gating predicate suppressed it).

- [ ] Add an additional case in the same file:
  - Mute room X for user A as `mute_mode = 'all'`; run tick; assert no send (DMs alone would have produced a send, but seed only the mentions in this subcase). Actually do this as a separate scenario to keep the assertion crisp.

### Task 5: Settings UI, admin defaults, and user-creation default

- [ ] Edit `server/src/routes/settings.rs` to add `notify_email_digest_enabled` to the form. Extend `db::auth::set_notification_prefs` from `(browser, sound, push)` to `(browser, sound, push, email_digest)`. Update every existing call site (the current signature has three args; the compiler will flag them).
- [ ] Edit `server/src/views/settings.rs` to add `email_available: bool`. Populate from `state.email_client.is_some()`.
- [ ] Edit `server/templates/settings/page.html` to add the fourth checkbox with conditional `disabled` and help text.
- [ ] Edit `server/src/db/auth::insert_user` to read `default_notify_email_digest` from `app_settings` at insert time and initialise the new user's `notify_email_digest_enabled` column to that value. Pass the settings pool into `insert_user` (the function currently takes only the auth pool; thread the settings pool through, or look up the default via a small helper in `db::settings` that just reads the key-value row).
- [ ] Add `server/tests/db_email_digest_default.rs`:
  - Insert user U1 with default = 0; assert `notify_email_digest_enabled = 0`.
  - Flip default to 1 via the helper.
  - Insert user U2; assert U2's column is 1; assert U1 is unchanged.

### Task 6: Test cross-checks and verification

- [ ] Every `tests/*.rs` that includes `auth/0009_push_subscriptions.sql` (or earlier) must also include `auth/0010_digest_columns.sql`. Likewise add `settings/0004_smtp_settings.sql` and `settings/0005_default_email_digest.sql` where the settings pool is opened.
- [ ] Run `just check` (validates both standalone and saas, plus clippy in both modes, plus fmt).
- [ ] Run `just test` and `just test-saas`.
- [ ] Run `just verify` and confirm a fresh boot does not panic when SMTP is unconfigured.
- [ ] Smoke (real SMTP, optional, operator-side):
  - Configure SMTP in the admin page. Click "Send test email." Confirm receipt.
  - Enable digest in `/settings`. Close all browser tabs to lets-chat. Wait 1h. Confirm an email arrives covering any mentions/DMs that landed in the meantime.

### Task 7: Documentation

- [ ] Update `README.md`:
  - Add a "Email digests" subsection under Features. Document: hourly tick, 1-hour quiet period, one digest per offline session, 7-day cap, 50-item cap, opt-in default (per-instance configurable by an admin), what is required (SMTP configured AND `LETS_CHAT_SECRET_KEY` set).
  - Add a note to the SMTP section: "After upgrading from a previous version of lets-chat, the previously stored SMTP password is cleared during migration (it was stored in plaintext). Re-enter it in the admin settings page after upgrade; the new storage encrypts at rest under `LETS_CHAT_SECRET_KEY`."
- [ ] `.env.standalone` and `.env.saas` do not currently document email; no change.

## Tests and verification

- `just check`, `just test`, `just test-saas`, `just verify` all pass.
- New tests:
  - `db_smtp_settings.rs`: encrypted round-trip, `password: None` preserves existing, `password: Some("")` clears.
  - `db_email_digest.rs`: eligibility query for offline/active/recently-active/already-digested users; self-reset after activity.
  - `email_digest_dispatch.rs`: end-to-end with `MockEmailClient`; subject, body sections, item ordering, mute-mode filtering, DM-from-`dm_read_state`, idempotence under two consecutive ticks.
  - `db_email_digest_default.rs`: admin-toggled default propagates to new users only.
  - `routes_ws_bump.rs`: WS connection bumps `last_ws_seen_at`; outbound `Mentioned` within 5min does not re-bump.
  - `routes_admin_smtp_test.rs`: "Send test email" button hits the mock client.
- Manual smoke as in Task 6.

## Things to confirm during implementation

1. **Lettre 0.11 feature names.** `tokio1-rustls-tls` is the documented name as of this writing, but Lettre's feature flags have shifted between minor versions. Verify against the actual resolved version and adjust if needed. If async `AsyncSmtpTransport` is easier to weave into tokio than the sync `SmtpTransport::send` in a `spawn_blocking`, swap the implementation; the trait does not care.
2. **WS frame-send call sites for the throttled bump.** The plan assumes there is exactly one outbound-`Mentioned` send site in `routes/ws.rs`. Confirm during implementation; if there are multiple, factor the throttle into a `WsConn::on_mentioned_send(&self, state)` helper so the throttle logic does not duplicate.
3. **Email address column on users.** Confirm `users.email` exists in the auth schema and is reliably populated for at least admin users. If `email` is optional and many existing users have it NULL, the digest is a no-op for those users (eligibility filter excludes them), which is correct. The "Send test email" button must explicitly check the admin has an email and surface a clear error if not, rather than panicking.
4. **`dm_notification_settings` columns.** The plan assumes `(user_id, peer_user_id, mute_mode)` from phase 17. Confirm column names before writing the eligibility SQL; if the column is `target_user_id` or similar, adjust.
5. **Auth-pool / chat-pool join shape.** The digest eligibility query is split across pools because SQLite has no cross-database joins. The plan uses one query per pool plus a bulk username lookup. If a `ATTACH DATABASE` approach has been adopted elsewhere in the codebase for similar joins, prefer that for consistency; if not, the two-query pattern matches phase 19's precedent.

## Summary plan

Phase 22 ships email digests of missed mentions and DMs, closing the offline-user gap in the notification stack. Seven tasks:

1. **WS-bump precursor.** Add `last_ws_seen_at`, `notify_email_digest_enabled`, `last_digest_sent_at` columns and a partial index. Bump `last_ws_seen_at` on WS connection-open and on outbound `Mentioned` (throttled per-connection at 5min). Idle-flip semantics deliberately unchanged.
2. **SMTP infrastructure.** Add `lettre` (rustls), build a typed `smtp_settings` table with encrypted password (reusing `crate::crypto::seal`), migrate from the existing key-value rows (discarding the plaintext password), build the `EmailClient` trait with `LettreEmailClient` and `MockEmailClient`, ship a "Send test email" button on the admin page, and surface SMTP-unconfigured banners.
3. **Digest content.** View models, HTML and plaintext Askama templates, snippet helper (140 chars, word-truncated, HTML-escaped + mention-bolded for HTML).
4. **Eligibility query and digest tick.** Per-user "missed" predicate combining `MAX(last_active_at, last_ws_seen_at)` with the 1-hour quiet period and the self-resetting `last_digest_sent_at` gate. Hourly background tick in `main.rs` modeled on `spawn_idle_scanner`.
5. **Settings UI and admin default.** Fourth notification checkbox at `/settings`; admin toggle for the per-instance default (opt-in default = 0); user-registration path reads the default.
6. **Test cross-checks and verification.** `just check`, `just test`, `just test-saas`, `just verify`; new test files for each new module.
7. **Documentation.** README additions covering cadence, quiet period, opt-in default, the 7-day cap, the SMTP password re-entry requirement after upgrade.

After this phase, the notification stack covers: in-app (phase 14), per-room/per-DM mute (phases 15, 17), Web Push (phase 16), and email digest for the offline case (this phase). The four transports share `ChatEvent::Mentioned` and the `mentions` / `dm_read_state` tables as the source of truth; each transport's dispatch path remains inline at its call site. The "unified notification dispatch service" abstraction is still not warranted.
