# Phase 13 - File & Image Uploads (+ LC-3 link previews)

## Goal

Let signed-in users attach a file or image to a message and have it render
inline alongside the message body, server-side rendered like every other
fragment in the app. Allow images, GIFs, WebP, and PDF; enforce a configurable
size limit; store everything on the local filesystem under
`${LETS_CHAT_DATA_DIR}/uploads/` with a content-addressed (sha256) layout that
naturally deduplicates and stays portable to a future S3 backend.

LC-3 piggybacks one extra requirement: when a message body contains a URL,
render the URL as an inline anchor and lazy-load a preview card (title,
description, image) below the message via an unfurl endpoint that fetches the
target page server-side, parses OpenGraph/Twitter/`<title>`, and is hardened
against SSRF.

## Architecture

- **Stack** (current truth, not the stale TODO sections): Axum 0.8 + Askama
  templates + HTMX. The router builds a single `Router` in
  `server/src/routes/mod.rs::build_router`. Three SQLite pools live in
  `AppState`. WebSocket payloads are pre-rendered HTML fragments tagged with
  `hx-swap-oob`, never JSON.
- **Upload flow**:
  1. Composer `<input type="file">` posts to `POST /api/upload` (multipart)
     **before** the message is sent. The endpoint streams the bytes to a temp
     file under `${LETS_CHAT_DATA_DIR}/uploads/.tmp/{uuid}`, counting bytes
     against `max_upload_bytes` (10 MiB default, read from `settings.db`).
  2. After streaming, `infer::get_from_path` sniffs magic bytes and validates
     against the allowlist (`image/jpeg`, `image/png`, `image/gif`,
     `image/webp`, `application/pdf`).
  3. Sha256 of the temp file becomes the canonical name
     `${LETS_CHAT_DATA_DIR}/uploads/{sha256}.{ext}`. If the destination already
     exists, the temp file is removed and the existing path is reused
     (de-dup). A row in `file_uploads` is inserted with `message_id = NULL`
     and `uploader_id = caller`.
  4. Endpoint replies `{ "file_id": i64, "url": "/api/files/{id}" }`.
  5. The composer hides a `file_id` input when the upload completes. The
     existing message-send `POST /room/{id}/messages` accepts an optional
     `file_id`; the handler validates that the caller owns the orphan upload
     and links it to the new message in a single update.
- **Serve flow**: `GET /api/files/:id` looks up the upload row, resolves
  `message_id -> room_id`, runs the same `is_room_accessible` predicate every
  message handler uses, and streams the file via
  `tokio::fs::File` + `tokio_util::io::ReaderStream` +
  `axum::body::Body::from_stream` with the **sniffed** Content-Type and
  `Content-Disposition: inline`. Orphan uploads (`message_id IS NULL`) are
  fetchable only by the original uploader.
- **Render path** (single render = the server): a new
  `templates/partials/attachment.html` partial is included from
  `room/message.html` for every message that has a non-empty `attachments`
  vec. The same partial is reused by the WS broadcast path because both first
  paint and live updates render `room/message.html`.
- **Link previews**: at render time, the message body is scanned with
  `linkify`. Each URL is rendered as an `<a>`. After the **first** URL only,
  a `<div hx-get="/api/unfurl?url=..." hx-trigger="load" hx-swap="innerHTML">`
  shell is appended. The unfurl endpoint fetches the URL with `reqwest` (5s
  timeout, 1 MiB body cap, http/https only, custom resolver that rejects
  loopback/RFC1918/link-local/non-global IPs to block SSRF), parses
  OpenGraph + Twitter card + `<title>` with `scraper`, and renders an Askama
  preview-card fragment.

## Tech Stack

- New crates: `infer` (magic-byte sniffing), `tokio-util` (`StreamReader` /
  `ReaderStream` for streaming), `sha2` (content-addressing), `linkify`
  (URL extraction from message bodies), `reqwest` + `scraper` (link preview
  fetching/parsing).
- Multipart support is already enabled on `axum = "0.8"` via the `multipart`
  feature in `server/Cargo.toml` - confirmed; no extra dep needed there.
- Local filesystem storage via `tokio::fs`. The `storage_path` column stays
  TEXT so a future S3/object-store swap is a non-breaking migration.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Add | `server/migrations/chat/0011_uploads.sql` | `file_uploads` table + indexes (orphan GC index on `created_at` where `message_id IS NULL`). |
| Add | `server/migrations/settings/0002_uploads.sql` | Seed `max_upload_bytes = 10485760`. |
| Add | `server/migrations/chat/0012_link_previews.sql` | `link_previews` cache (URL hash, OG fields, fetched_at). |
| Add | `server/src/models/attachment.rs` | `Attachment { id, filename, mime_type, size_bytes, url }` (the client-facing url is `/api/files/{id}`). |
| Edit | `server/src/models/mod.rs` | Re-export `Attachment`. |
| Add | `server/src/db/uploads.rs` | `insert_upload`, `link_upload_to_message`, `get_upload`, `attachments_for_messages` bulk loader. |
| Edit | `server/src/db/mod.rs` | `pub mod uploads;` plus an `uploads_dir()` helper analogous to `avatars_dir()`. |
| Edit | `server/src/views/room.rs` | Add `attachments: Vec<Attachment>` to `MessageView`. |
| Add | `server/src/routes/uploads.rs` | `POST /api/upload`, `GET /api/files/:id`. |
| Add | `server/src/routes/unfurl.rs` | `GET /api/unfurl?url=...` - SSRF-hardened link preview. |
| Edit | `server/src/routes/mod.rs` | `mod uploads; mod unfurl;` plus `.route(...)` lines in `build_router`. |
| Edit | `server/src/routes/room.rs` | `MessageForm` accepts optional `file_id`; handler validates ownership and calls `link_upload_to_message`. Bulk-load attachments when listing messages. |
| Edit | `server/src/routes/dm.rs` | Bulk-load attachments when listing DM messages. |
| Edit | `server/src/routes/ws.rs` | `render_new_message` and `render_edited_message` populate `attachments` for the rendered MessageView. |
| Add | `server/templates/partials/attachment.html` | Render `<img>` for image MIME types, "download card" for PDFs. |
| Add | `server/templates/partials/link_preview.html` | Card markup for unfurl response. |
| Edit | `server/templates/room/message.html` | Include `partials/attachment.html` when attachments present; bypass message bubble when body is empty + exactly one image attachment; scan body for URLs with `linkify` and render anchors + lazy preview shell. |
| Edit | `server/templates/room/composer.html` | Add paperclip button, hidden `<input type="file">`, hidden `<input name="file_id">`, staged-chip, immediate-upload glue (under 30 lines of inline JS). |
| Add | `server/tests/db_uploads.rs` | DB-level: insert, link, retrieve, access-control denial for non-DM-member. |
| Add | `server/tests/routes_uploads.rs` | HTTP: 200 happy path, 413 oversize, 415 wrong MIME, 401 anonymous, 403 cross-room, send-with-attachment renders fragment. |
| Edit | `server/Cargo.toml` | Add `infer`, `tokio-util` (`io` feature), `sha2`, `linkify`, `reqwest` (rustls-tls, gzip, json), `scraper`. |

## Tasks

### Task 1 - Dependencies & migrations

- [ ] Add new dependencies to `server/Cargo.toml` (workspace versions are not
      currently set for these, so pin in the crate).
  - `infer = "0.16"`
  - `tokio-util = { version = "0.7", features = ["io"] }`
  - `sha2 = "0.10"`
  - `linkify = "0.10"`
  - `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "gzip", "stream"] }`
  - `scraper = "0.20"`
  - `url = "2"`
- [ ] Verify `axum = "0.8"` already declares `multipart` (it does in the
      current Cargo.toml; do not duplicate).
- [ ] Confirm next chat migration number: `ls server/migrations/chat/`
      shows up to `0010_room_name_per_enclave.sql`, so the next chat
      migration is **`0011`**, not `0009` as TODO.md hinted. Note this in
      the plan and the commit message.
- [ ] Create `server/migrations/chat/0011_uploads.sql`:

```sql
CREATE TABLE IF NOT EXISTS file_uploads (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    uploader_id  TEXT NOT NULL,
    message_id   INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    filename     TEXT NOT NULL,
    mime_type    TEXT NOT NULL,
    size_bytes   INTEGER NOT NULL,
    storage_path TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_file_uploads_message ON file_uploads(message_id);
-- Orphan GC: a future cron sweeps rows with message_id IS NULL older than N minutes.
CREATE INDEX IF NOT EXISTS idx_file_uploads_orphan ON file_uploads(created_at) WHERE message_id IS NULL;
```

- [ ] Create `server/migrations/chat/0012_link_previews.sql`:

```sql
CREATE TABLE IF NOT EXISTS link_previews (
    url_hash    TEXT PRIMARY KEY,           -- sha256 hex of normalized URL
    url         TEXT NOT NULL,
    title       TEXT,
    description TEXT,
    image_url   TEXT,
    fetched_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
```

- [ ] Create `server/migrations/settings/0002_uploads.sql`:

```sql
INSERT OR IGNORE INTO settings (key, value) VALUES
    ('max_upload_bytes', '10485760');
```

- [ ] No new wiring is needed in `server/src/db/mod.rs` for the migration
      runners themselves - `sqlx::migrate!("./migrations/chat")` and
      `sqlx::migrate!("./migrations/settings")` pick up new files
      automatically. **Do** add a small `uploads_dir()` helper next to
      `avatars_dir()`:

```rust
pub fn uploads_dir() -> PathBuf {
    let p = PathBuf::from(data_dir()).join("uploads");
    if let Err(e) = std::fs::create_dir_all(&p) {
        tracing::warn!(error = %e, path = %p.display(), "failed to create uploads dir");
    }
    p
}
```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git checkout -b feat/uploads`
- [ ] `git add server/Cargo.toml Cargo.lock server/migrations/chat/0011_uploads.sql server/migrations/chat/0012_link_previews.sql server/migrations/settings/0002_uploads.sql server/src/db/mod.rs`
- [ ] `git commit -m "feat(uploads): schema + deps for file uploads and link previews"`

### Task 2 - Models & DB layer

- [ ] `server/src/models/attachment.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: i64,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    /// Client-facing fetch URL (`/api/files/{id}`). Never expose `storage_path`.
    pub url: String,
}
```

- [ ] Re-export from `server/src/models/mod.rs`: `pub mod attachment; pub use attachment::Attachment;`.
- [ ] `server/src/db/uploads.rs`: `UploadRow`, `insert_upload`,
      `link_upload_to_message`, `get_upload(id) -> Option<(UploadRow, Option<i64> /* room_id */)>`,
      `attachments_for_messages(pool, &[i64]) -> HashMap<i64, Vec<Attachment>>`.
      `get_upload` joins `file_uploads -> messages` so the route layer can
      gate access in one query.
- [ ] `pub mod uploads;` in `server/src/db/mod.rs`.
- [ ] Add `pub attachments: Vec<Attachment>` to
      `server/src/views/room.rs::MessageView`. Default to `Vec::new()` at
      every construction site.
- [ ] Update every `MessageView { ... }` literal to set `attachments: Vec::new()`
      (or to populate from a bulk lookup in `routes/room.rs::get_room` and
      `routes/dm.rs::get_dm`):
  - `routes/room.rs::get_room`, `get_single_message`, `patch_message`
  - `routes/dm.rs::get_dm`
  - `routes/ws.rs::render_new_message`, `render_edited_message`
- [ ] In `routes/room.rs::get_room` and `routes/dm.rs::get_dm`, after
      collecting `raw_messages` call
      `db::uploads::attachments_for_messages(&state.chat, &ids)` once and
      attach the per-message vec.
- [ ] In `routes/ws.rs::render_new_message` and `render_edited_message`,
      load attachments for that single message id.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/models/attachment.rs server/src/models/mod.rs server/src/db/uploads.rs server/src/db/mod.rs server/src/views/room.rs server/src/routes/room.rs server/src/routes/dm.rs server/src/routes/ws.rs`
- [ ] `git commit -m "feat(uploads): Attachment model + DB layer + MessageView wiring"`

### Task 3 - POST /api/upload (multipart, streaming, sniffing)

- [ ] Create `server/src/routes/uploads.rs`. Handler:
  - extractor: `AuthUser`.
  - read `max_upload_bytes` from `settings.db` (`db::settings::get_setting`),
    default to `10_485_760` if missing or unparseable.
  - iterate `axum::extract::Multipart::next_field()`. For the first field
    named `file`, write chunks to
    `${LETS_CHAT_DATA_DIR}/uploads/.tmp/{uuid}` while accumulating byte
    count. On overflow: `tokio::fs::remove_file(&tmp).await` then return
    `StatusCode::PAYLOAD_TOO_LARGE`.
  - after streaming: `let kind = infer::get_from_path(&tmp).await?;` -
    validate against allowlist. Reject `415` otherwise. Map kind to a
    canonical extension (`jpg|png|gif|webp|pdf`) so downloaded names match
    the sniffed type, even if the user-supplied filename lied.
  - sha256 the temp file (re-open + stream), rename to
    `${LETS_CHAT_DATA_DIR}/uploads/{hex}.{ext}`. If the destination already
    exists, just delete the temp file (de-dup).
  - `db::uploads::insert_upload(...)` with `message_id = NULL`,
    `uploader_id = user.id`, original `filename`, sniffed `mime_type`,
    `size_bytes`, `storage_path = "{hex}.{ext}"` (relative; absolutize at
    serve time via `uploads_dir().join(...)`).
  - return `axum::Json(serde_json::json!({ "file_id": id, "url": format!("/api/files/{id}") }))`.
- [ ] Mount `POST /api/upload` in `routes::build_router`.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/routes/uploads.rs server/src/routes/mod.rs`
- [ ] `git commit -m "feat(uploads): POST /api/upload with streaming + magic-byte validation"`

### Task 4 - GET /api/files/:id

- [ ] In `server/src/routes/uploads.rs` add `serve_file(...)`:
  - extractor: `OptionalUser`. If `None`, return `401`.
  - `db::uploads::get_upload(state.chat, id)` returns `(UploadRow, Option<i64> /* room_id */)`.
    `None` from the lookup -> `404`.
  - if `room_id` is `Some(rid)`: call
    `db::chat::is_room_accessible(&state.chat, rid, &user.id, is_admin)`;
    `false` -> `403`.
  - if `room_id` is `None` (orphan): require `row.uploader_id == user.id`,
    else `403`.
  - open the file via `tokio::fs::File::open(uploads_dir().join(&row.storage_path))`.
    If missing, `500` (the row should always have a backing file).
  - build response:
    - `Content-Type` = `row.mime_type` (sniffed at upload time, trusted).
    - `Content-Disposition` = `inline; filename="{row.filename}"` with
      `percent-encoding` for non-ASCII filenames if necessary - keep it
      simple, encode quotes/control chars defensively.
    - body = `axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file))`.
- [ ] Mount `GET /api/files/{id}` in `routes::build_router`.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/routes/uploads.rs server/src/routes/mod.rs`
- [ ] `git commit -m "feat(uploads): GET /api/files/:id with auth-gated streaming"`

### Task 5 - Send-message-with-attachment integration

- [ ] In `server/src/routes/room.rs::MessageForm`, add `pub file_id:
      Option<i64>`. Allow blank body when `file_id.is_some()` (image-only
      messages are valid).
- [ ] In `post_message`:
  - if `file_id.is_some()`, `db::uploads::get_upload(...)` and reject
    unless `row.uploader_id == user.id && row.message_id is None`.
  - after `insert_message`, call
    `db::uploads::link_upload_to_message(&state.chat, file_id, new_id)`.
  - load `attachments` for the new message and attach to the `Message` /
    `MessageView` used by the broadcast path. Easiest: after insert, fetch
    the single attachment vec and pass it through the existing event flow.
- [ ] Confirm DM messages route through the same `post_message`. They do -
      `routes/dm.rs::get_dm` uses `room/composer.html`, which posts to
      `/room/{room_id}/messages`. No DM-specific handler change needed.
- [ ] **Author-name + attachment in the `ChatEvent`**: the `Message` struct
      in `models/message.rs` currently has no attachments. Two options:
  1. Add `attachments: Vec<Attachment>` to `models::Message` (preferred -
     keeps the broadcast self-contained).
  2. Re-fetch attachments in the WS render path. (Simpler diff but extra
     query per recipient.) Choose option 1 to keep render fast; subscribers
     just clone the vec from the broadcast event.
- [ ] Update `Message` in `server/src/models/message.rs` to carry
      `attachments: Vec<Attachment>` (default empty). Update every
      `Message { ... }` literal accordingly.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo test -p lets-chat-server` (existing tests must stay green)
- [ ] `git add server/src/routes/room.rs server/src/models/message.rs server/src/routes/ws.rs`
- [ ] `git commit -m "feat(uploads): link uploaded files to messages on send"`

### Task 6 - Composer UI (paperclip + staged chip)

- [ ] Edit `server/templates/room/composer.html`. Add paperclip button +
      `<input type="file" name="file" hidden>` + hidden `<input
      type="hidden" name="file_id">` + a chip `<div id="lc-staged">`.
- [ ] Inline JS (~30 lines max): on file change, fetch
      `POST /api/upload` with a `FormData`, parse `{file_id, url}`, set the
      hidden input value, render the chip with filename + size + clear
      button. On submit, the existing form already POSTs the message with
      the new `file_id` field included. After successful submit, clear the
      file_id input and chip.
- [ ] Render inline error text for 413 / 415 by reading the response status
      and message.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/templates/room/composer.html`
- [ ] `git commit -m "feat(uploads): composer paperclip + staged-attachment chip"`

### Task 7 - Attachment partial + message rendering

- [ ] Create `server/templates/partials/attachment.html`. Image branch:
      `<img src="{{ a.url }}" alt="{{ a.filename }}" class="max-w-sm
      max-h-96 rounded mt-1">`. Non-image (PDF) branch: a small download
      card linking to `a.url` with filename + size.
- [ ] Edit `server/templates/room/message.html`:
  - Detect "image-only" path: `body.is_empty() &&
    attachments.len() == 1 && attachments[0].mime_type.starts_with("image/")`.
    In that case render only the image inside the message bubble shell
    (skip the body `<div>` and bubble background).
  - Else: render body, then `{% for a in message.attachments %}{% include
    "partials/attachment.html" %}{% endfor %}`.
- [ ] Confirm the same partial is used by the WS new-message broadcast
      (since `ws/new_message.html` already `{% include
      "room/message.html" %}`, this is automatic).
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/templates/partials/attachment.html server/templates/room/message.html`
- [ ] `git commit -m "feat(uploads): render attachments inline in message fragment"`

### Task 8 - Link previews (LC-3)

- [ ] Create `server/src/routes/unfurl.rs`. `GET /api/unfurl?url=...`:
  - require `AuthUser` (don't expose unfurling anonymously - protects
    against abuse).
  - parse via `url::Url`. Reject if scheme is not `http`/`https`. 400.
  - SSRF: build a `reqwest::Client` with a custom DNS resolver that, after
    resolving, rejects:
    - loopback (`127.0.0.0/8`, `::1`)
    - RFC1918 (`10/8`, `172.16/12`, `192.168/16`)
    - link-local (`169.254/16`, `fe80::/10`)
    - any non-globally-routable IP (CG-NAT, multicast, etc.)
  - 5s timeout, max 1 MiB read. Refuse non-`text/html` Content-Types.
  - parse with `scraper`: prefer `<meta property="og:title">`, fall back to
    `<meta name="twitter:title">`, then `<title>`. Same precedence for
    description and image.
  - cache lookup: hash the *normalized* URL (lowercase scheme + host;
    path/query preserved), `db::uploads::get_link_preview(hash)`. If hit
    and < 24h old, render directly. Else fetch + insert + render.
  - render `partials/link_preview.html` and return as Html (NOT JSON -
    HTMX swaps the response straight in).
- [ ] Add `GET /api/unfurl` to `routes::build_router`.
- [ ] Edit `server/templates/room/message.html`:
  - In the body block, scan `message.body` with `linkify`. Server-side, do
    it in Rust by extending `MessageView` with a `body_segments: Vec<BodySegment>`
    helper field (text vs link), or render the linkified body via a small
    Askama filter. Simpler: precompute in
    `routes/room.rs::get_room` (and dm/ws variants) into a `Vec<BodySegment>`
    on the view; the template iterates and emits `<a>` for links and text
    for plain segments.
  - Append `<div hx-get="/api/unfurl?url={url}" hx-trigger="load"
    hx-swap="innerHTML"></div>` for the **first** URL only.
- [ ] Decision: I'll keep the cache table because preview fetches are
      slow and re-fetching every render burns the whole page on slow
      backends. 24h TTL.
- [ ] Add `db::uploads::get_link_preview(...)` and
      `db::uploads::upsert_link_preview(...)` (or a new `db/previews.rs`).
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git checkout -b feat/link-previews` (if keeping uploads commits
      separate; otherwise stay on the same branch and tag the commit
      `feat(previews):`).
- [ ] `git add ...`
- [ ] `git commit -m "feat(previews): SSRF-hardened /api/unfurl + lazy preview cards"`

### Task 9 - Integration tests

- [ ] `server/tests/db_uploads.rs` - mirror `db_dm.rs` setup helpers:
  - `insert_upload` returns id; `get_upload` round-trips fields.
  - `link_upload_to_message` flips `message_id` from NULL.
  - `attachments_for_messages` returns the right vec keyed by message_id.
  - access denial: a non-DM-member calling `is_room_accessible` for a DM
    they aren't in returns false (sanity check the integration).
- [ ] `server/tests/routes_uploads.rs` - mirror `routes_enclave.rs::app_with_user`:
  - extend `open_pool("chat")` and `open_pool("settings")` to include
    migration `0011`/`0012` and `0002`.
  - `POST /api/upload` with a small valid PNG body returns 200 + JSON
    with `file_id`.
  - 13 MiB body returns 413.
  - body whose magic bytes are a ZIP renamed `.png` returns 415.
  - anonymous request to `POST /api/upload` returns 401 (the AuthUser
    extractor redirects to /login - assert the redirect or status as the
    pattern dictates).
  - cross-room: user A uploads + sends in room R1; user B (not a member of
    a private room) requests `/api/files/{id}` -> 403.
  - end-to-end: upload, then `POST /room/{rid}/messages` with `file_id`,
    then `GET /room/{rid}` returns HTML containing the `<img>` partial.
- [ ] Add a small fixture PNG (8 bytes minimum sniffable header) - inline
      it as a byte literal in the test.
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/tests/db_uploads.rs server/tests/routes_uploads.rs`
- [ ] `git commit -m "feat(uploads): integration tests for upload + serve + send"`

### Task 10 - Final verification

- [ ] `just check-server`
- [ ] `just check-clippy` - if warnings appear, append `-- -D warnings` in
      a one-shot invocation (`./dev/cargo clippy -p lets-chat-server -- -D
      warnings`) and fix.
- [ ] `just test`
- [ ] `just check-fmt` (run `just fmt` if it fails)
- [ ] `just verify` - smoke that the release binary still serves `/login`.
- [ ] Manual smoke (`just dev-web-local` -> `http://localhost:18080`):
  - upload a PNG in `#general`, see it render inline.
  - paste `https://example.com` in a message, see preview card swap in.
  - try a 12 MB file -> 413 chip error.
  - rename a `.zip` to `.png` and upload -> 415 chip error.
- [ ] Open PR: `feat(uploads): file/image attachments + link previews
      (Phase 13)`. Body lists the new endpoints, env vars, and notes that
      the orphan upload GC is documented but not yet scheduled.

## Out of scope

- S3 / object-store backend.
- Image thumbnailing, EXIF stripping. (`storage_path` is opaque enough
  that a future migration can fan out to thumbnail variants.)
- Drag-and-drop upload. (Paperclip-button only.)
- Background GC of orphan uploads. (Index + TODO comment only.)
- Audio / video attachments.

## Things to confirm / deviations

- Migration number: TODO.md said `0009`. Actual next is **`0011`**
  (chat already has 0001-0010). Spec was stale.
- Existing room-access helper: `db::chat::is_room_accessible` is already a
  shared helper. No extraction needed.
- Hub API: `state.hub.broadcast_to_user(user_id, &ChatEvent)` and
  `broadcast_to_room`. Match it; do not introduce a new shape.
- `MessageView` already feeds every template - threading `attachments`
  through it is the only fan-out required.
