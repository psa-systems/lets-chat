# Phase 26 - Message Edit History

## Goal

Persist prior message bodies on every edit. Surface them in a right-side `#history-panel` drawer reached by clicking the existing `(edited)` badge. Render versions oldest-first with the current body labelled "Current" at the bottom. Prior bodies render through the existing markdown pipeline so chips, code blocks, custom emoji, and uploaded-image links work identically to live messages. The phase ships no diff view, no retention policy, no admin-edit capability, and no mod-audit endpoint - those are all explicit follow-ups, named in §Out of scope.

## Phase shape: predict-first

The full shape is mechanical from reading the codebase: one new table, one new index, one new query, one new handler, one new template pair, plus the existing `(edited)` span becomes a `<button>`. There is no exploratory step. All eight tasks could be done in one sitting; they are split for review purposes.

## Hard constraints

- No new dependencies. Stays on Askama + HTMX + inline IIFEs. The drawer mirrors `server/templates/room/thread_panel.html` (right-side `<aside>`, close-button shape, slot replacement via `hx-swap="outerHTML"`).
- Markdown renderer unchanged. Prior bodies render via the existing `views::markdown::render(body, mentions, emojis)` entry point at `server/src/views/markdown.rs:54`.
- Mention resolution against current usernames, not a snapshot at edit time. Same behavior as live message rendering. If "bob" became "robert" between the edit and the history view, the prior body shows "robert". Documented decision; matches Slack.
- WS broadcast surface unchanged. The existing `MessageEdited` event continues to push new body + edited_at. The history drawer is an explicit HTTP fetch on click; nothing pushes into the drawer.
- Soft-deleted message hides history. Reuse `db::chat::get_message` (which already filters `deleted_at IS NULL`) as the gate. No widening for v1.
- Author-only editing today means `editor_user_id` is not stored. Forward-compat migration documented in §Schema below.
- Claude does NOT commit or push. Stage with `git add` and stop. The user commits per task during execution as a review step.

## Out of scope

- Diff view between versions (Slack-style highlight of changed words). Distinct feature; needs a markdown-aware diff library plus escape handling. Separate ticket.
- Edit retention / trimming policy. Acknowledged open question; defer until storage growth is observed in production.
- Admin / moderator edit capability. Until that lands, "editor identity disclosure" has no surface to disclose.
- Mod-audit access to deleted-message history. Different code path (`get_message_for_moderation` + `/admin/audit/*`), explicitly not a reuse of the user-facing endpoint.
- Push proactive history-panel updates. If a viewer keeps the drawer open and a fresh edit lands, the drawer is stale until reopen. Acceptable for v1; matches Slack.
- DM-thread-specific history. Threads live in the same `messages` table via `parent_id` (phase 11); the new INSERT path covers replies for free with no thread-specific code.
- Re-architecting `update_message_body`'s caller contract. The existing handler at `routes/room.rs:742` keeps the same call shape; only the function's internals change.

## Background

### Storage today

`messages.body TEXT NOT NULL` is mutated in place by `update_message_body` (`server/src/db/chat.rs:272`) which runs `UPDATE messages SET body = ?, edited_at = ? WHERE id = ?`. Prior content is overwritten with no trail. `edited_at` records the timestamp of the most recent edit and drives the `(edited)` badge in `server/templates/room/message.html`.

### Edit handler today

`PATCH /messages/:id` (`server/src/routes/room.rs:725`) is author-only; non-author receives `AppError::Forbidden`. Flow:

1. `get_message` (rejects soft-deleted)
2. Authorize (`m.user_id == user.id`)
3. Trim, reject empty
4. `update_message_body` runs the single UPDATE
5. Broadcast `MessageEdited` over the hub
6. Reconcile mentions (non-DM rooms)
7. Re-render the message fragment and return

Phase 26 wraps step 4 with `INSERT INTO message_edits` so the prior body is captured before the UPDATE overwrites it. Steps 5-7 are untouched. The reconcile in step 6 is intentionally NOT inside the new transaction: today a reconcile failure does not roll back the body change, and widening that scope is out of phase.

### Cross-pool reference convention (phase 14)

`mentions.message_id INTEGER REFERENCES messages(id) ON DELETE CASCADE` for the in-pool FK. User references (`mentioned_user_id`, `author_user_id`) are plain `TEXT NOT NULL` with no FK because users live in `auth.db`. The `message_edits` table mirrors this exactly.

### Timestamp resolution caveat

`edited_at` is written with `format("%Y-%m-%d %H:%M:%S")` (no sub-second component). Two edits in the same second on the same message produce identical timestamps. This rules out `(message_id, edited_at)` as a composite primary key. The phase uses a rowid PK plus an explicit `idx_message_edits_message_id` index on `(message_id, edited_at)` for read paths. Rapid-edit collisions become two distinct rows with identical display timestamps, which is correct.

### Drawer slot convention (phase 7)

`server/templates/room/thread_panel_closed.html` is `<aside id="thread-panel" class="hidden"></aside>`. The open template is `server/templates/room/thread_panel.html`. Routes: `GET /room/:room_id/thread/:message_id` returns the open panel; `DELETE /thread-panel` returns the closed (empty) version. Both `room/page.html` and `dm/page.html` carry an `<aside id="thread-panel">` slot.

Phase 26 adds an exactly parallel `<aside id="history-panel">` slot and uses the same open/close routing shape. Two slots, sibling `<aside>` elements; neither replaces the other.

### Test pool helpers (phase 24 cleanup + remaining drift)

`server/tests/common/mod.rs` exposes `chat_pool()` driven by `sqlx::migrate!("./migrations/chat")`, which auto-picks-up new migration files. Test binaries that use that helper need no change.

But many test binaries still hand-roll their pools with explicit `include_str!(...)` migration lists - the "migration-list drift" hazard documented in `CLAUDE.md`. Enumerate hand-roll sites with:

```nu
grep --recursive --line-number "0024_voice_channel_flag.sql" server/tests/
```

Two coexisting patterns: array (`for sql in [include_str!(...), ...]`) and verbose (`let chat_mN = include_str!(...); sqlx::raw_sql(chat_mN).execute(&pool).await.expect("chat migration N");`). Add the new migration in the matching shape per site. Phase 24 left both shapes in place intentionally; do not consolidate as part of this phase.

## Schema

### New migration `server/migrations/chat/0025_message_edits.sql`

```sql
-- Phase 26: per-message edit history. Each row captures the body that was
-- displaced by an edit, plus the timestamp of the edit that displaced it.
-- The current body lives in messages.body; rendering history reads N rows
-- here in (edited_at, id) order and appends the current body as the tail.
--
-- editor_user_id is intentionally not stored: today PATCH /messages/:id
-- is author-only, so the editor is always (SELECT user_id FROM messages
-- WHERE id = message_edits.message_id). When admin/mod edit capability
-- lands, add the column and backfill existing rows with that subquery.

CREATE TABLE IF NOT EXISTS message_edits (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id      INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    previous_body   TEXT NOT NULL,
    edited_at       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_message_edits_message_id
    ON message_edits (message_id, edited_at);
```

The comment block is load-bearing forward-compat context - keep it. The future admin-edit migration will be:

```sql
ALTER TABLE message_edits ADD COLUMN editor_user_id TEXT NOT NULL DEFAULT '';
UPDATE message_edits
   SET editor_user_id = (SELECT user_id FROM messages WHERE messages.id = message_edits.message_id);
```

### Row insertion semantics

The INSERT captures `previous_body = messages.body` (the body the user is about to overwrite) and `edited_at = <new edit's timestamp>`. The row therefore reads as "at this timestamp the displayed body changed from previous_body to whatever came next." Querying `ORDER BY edited_at ASC, id ASC` gives versions in the order they existed. Append `messages.body` (with `messages.edited_at`) as the "Current" tail.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Add | `docs/superpowers/plans/2026-05-17-phase26-edit-history.md` | This plan. |
| Add | `server/migrations/chat/0025_message_edits.sql` | Schema for prior-version storage. |
| Edit | `server/src/db/chat.rs` | `update_message_body` opens a transaction, captures prior body, inserts a `message_edits` row, runs the existing UPDATE, commits. New `list_message_edits` reader for the history endpoint. New `MessageEdit` row struct. |
| Edit | `server/src/routes/room.rs` | New `get_history_panel` handler and `close_history_panel` handler. |
| Edit | `server/src/routes/mod.rs` | Register `GET /messages/{message_id}/history` and `DELETE /history-panel`. |
| Edit | `server/src/views/room.rs` | `HistoryPanelFragment` template struct + `HistoryEntryView { body_html, edited_at, label_kind }`. Place near `EditFormFragment` for locality. |
| Add | `server/templates/room/history_panel.html` | Drawer markup, mirrors `thread_panel.html` close-button shape. Renders pre-escaped body HTML from `HistoryEntryView`. |
| Add | `server/templates/room/history_panel_closed.html` | Single line: `<aside id="history-panel" class="hidden"></aside>`. |
| Edit | `server/templates/room/message.html` | Make the `(edited)` span clickable. Wrap or replace with a `<button>` that targets `#history-panel`. Accessible name "View edit history". |
| Edit | `server/templates/room/page.html` and `server/templates/dm/page.html` | Add `<aside id="history-panel" class="hidden"></aside>` as a sibling of the existing `<aside id="thread-panel">`. |
| Edit | `server/tests/message_editing.rs` | Extend with edit-history row assertions (one edit produces one row; two edits produce two rows in chronological order). |
| Add | `server/tests/routes_message_edit_history.rs` | HTTP-level tests for the new endpoint: gate via `get_message`, fragment shape, soft-delete hiding, markdown rendering of prior bodies. |
| Edit | Hand-rolled `setup_*_pool()` sites under `server/tests/` per drift rule. | Append `include_str!("../migrations/chat/0025_message_edits.sql")` in matching shape (array vs. verbose) at every site that lists migrations explicitly. |

## Tasks

### Task 1 - Migration `0025_message_edits.sql`

- [ ] Create `server/migrations/chat/0025_message_edits.sql` with the schema in §Schema above. Include the full forward-compat comment block ahead of the table definition.
- [ ] `./dev/cargo check -p lets-chat-server` to confirm `sqlx::migrate!()` picks up the file at compile time.
- [ ] `git add server/migrations/chat/0025_message_edits.sql` and stop.

### Task 2 - Hand-rolled migration-list drift sweep

- [ ] `grep --recursive --line-number "0024_voice_channel_flag.sql" server/tests/` to enumerate hand-roll sites.
- [ ] For each site, read the surrounding lines to identify the pattern (array `for sql in [include_str!(...), ...]` vs. verbose `let chat_mN = include_str!(...); sqlx::raw_sql(chat_mN).execute(...).await.expect(...);`). Append the new migration in the matching shape immediately after the `0024_voice_channel_flag.sql` line. Array takes 1 line; verbose takes 2.
- [ ] `just test` to confirm compile and pass across affected test binaries. A missed site usually surfaces as `SqliteError ... table message_edits has no column ...` at runtime in a downstream HTTP test.
- [ ] `git add` the touched test files and stop.

### Task 3 - `update_message_body` becomes transactional + history reader

- [ ] In `server/src/db/chat.rs`, rewrite `update_message_body` keeping its current signature `(pool, message_id, new_body) -> Result<String, sqlx::Error>` so callers stay untouched. The new body:

    ```rust
    let edited_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut tx = pool.begin().await?;
    let prior_body: String = sqlx::query_scalar("SELECT body FROM messages WHERE id = ?")
        .bind(message_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO message_edits (message_id, previous_body, edited_at) VALUES (?, ?, ?)")
        .bind(message_id)
        .bind(&prior_body)
        .bind(&edited_at)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE messages SET body = ?, edited_at = ? WHERE id = ?")
        .bind(new_body)
        .bind(&edited_at)
        .bind(message_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(edited_at)
    ```

    The SELECT inside the tx (not a `prior_body` parameter pulled from the caller's earlier `get_message`) is deliberate: under a hypothetical concurrent-edit race the in-tx read is the source of truth, not a value the caller held before acquiring the write path. Default deferred `BEGIN` is fine - there is no correctness property at stake that requires `BEGIN IMMEDIATE` (the upload sweep needed it for a COUNT/DELETE race; edits have no analogue).

- [ ] Add a row struct near the other chat models. Define `MessageEdit` inline in `db/chat.rs` (matches the existing `RawMessage` location, no need for a new model file):

    ```rust
    #[derive(Debug, Clone)]
    pub struct MessageEdit {
        pub id: i64,
        pub message_id: i64,
        pub previous_body: String,
        pub edited_at: String,
    }
    ```

- [ ] Add `pub async fn list_message_edits(pool: &SqlitePool, message_id: i64) -> Result<Vec<MessageEdit>, sqlx::Error>` reading `id, message_id, previous_body, edited_at FROM message_edits WHERE message_id = ? ORDER BY edited_at ASC, id ASC`.
- [ ] `./dev/cargo check -p lets-chat-server` clean.
- [ ] `git add server/src/db/chat.rs` and stop.

### Task 4 - History route + handler + view

- [ ] In `server/src/views/room.rs`, add the template structs near `EditFormFragment`:

    ```rust
    pub enum HistoryEntryKind { Prior, Current }

    pub struct HistoryEntryView {
        pub body_html: String,
        pub edited_at: String,
        pub kind: HistoryEntryKind,
    }

    #[derive(Template)]
    #[template(path = "room/history_panel.html")]
    pub struct HistoryPanelFragment<'a> {
        pub message_id: i64,
        pub entries: &'a [HistoryEntryView],
    }
    ```

- [ ] In `server/src/routes/room.rs`, add `get_history_panel`. Permission gate: `get_message` (rejects soft-deleted) + `is_room_accessible` (room membership). Together they match the user-facing access model exactly - the same shape `get_thread_panel` uses (`server/src/routes/room.rs:871`).

    ```rust
    pub async fn get_history_panel(
        State(state): State<AppState>,
        AuthUser(user): AuthUser,
        Path(message_id): Path<i64>,
    ) -> Result<Html, AppError> {
        let m = db::chat::get_message(&state.chat, message_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let is_admin = user.role == "admin";
        if !db::chat::is_room_accessible(&state.chat, m.room_id, &user.id, is_admin).await? {
            return Err(AppError::Forbidden);
        }
        let edits = db::chat::list_message_edits(&state.chat, message_id).await?;
        let mentions = /* mention refs for this single message, same call as load_message_view_for_viewer */;
        let emojis = db::custom_emojis::refs_for_room(&state.chat, m.room_id).await?;
        let mut entries: Vec<HistoryEntryView> = edits
            .into_iter()
            .map(|e| HistoryEntryView {
                body_html: crate::views::markdown::render(&e.previous_body, &mentions, &emojis),
                edited_at: e.edited_at,
                kind: HistoryEntryKind::Prior,
            })
            .collect();
        entries.push(HistoryEntryView {
            body_html: crate::views::markdown::render(&m.body, &mentions, &emojis),
            edited_at: m.edited_at.clone().unwrap_or_default(),
            kind: HistoryEntryKind::Current,
        });
        html(&HistoryPanelFragment { message_id, entries: &entries })
    }
    ```

  The mention refs are loaded once for the message and reused across all entries. Since usernames are resolved at render time (not snapshotted at edit time), a single set of refs is the correct input for every prior body. Markdown cache means repeated identical bodies do not re-render.

- [ ] Add `close_history_panel` mirroring `close_thread_panel` (`server/src/routes/room.rs:1084`):

    ```rust
    pub async fn close_history_panel() -> Result<Html, AppError> {
        html(&HistoryPanelClosedFragment {})
    }
    ```

  Define `HistoryPanelClosedFragment` in `server/src/views/room.rs` pointing at `room/history_panel_closed.html`, mirroring `ThreadPanelClosedFragment`.

- [ ] In `server/src/routes/mod.rs`, register:

    ```rust
    .route("/messages/{message_id}/history", get(room::get_history_panel))
    .route("/history-panel", delete(room::close_history_panel))
    ```

- [ ] `./dev/cargo check -p lets-chat-server` clean.
- [ ] `git add server/src/routes/room.rs server/src/routes/mod.rs server/src/views/room.rs` and stop.

### Task 5 - Templates

- [ ] `server/templates/room/history_panel_closed.html`: single line `<aside id="history-panel" class="hidden"></aside>`.
- [ ] `server/templates/room/history_panel.html`: drawer markup mirroring `thread_panel.html` shape:

    ```html
    <aside id="history-panel" class="w-96 border-l border-slate-200 flex flex-col bg-white">
      <header class="flex items-center justify-between border-b border-slate-200 px-3 py-2">
        <div class="font-semibold">Edit history</div>
        <button
          hx-delete="/history-panel"
          hx-target="#history-panel"
          hx-swap="outerHTML"
          class="text-slate-400 hover:text-slate-700"
          aria-label="Close history"
        >&#10005;</button>
      </header>
      <div class="flex-1 overflow-y-auto px-3 py-2 space-y-3 text-sm">
        {% for entry in entries %}
        <div class="rounded border border-slate-200 p-2">
          <div class="text-xs text-slate-500 mb-1">
            {% match entry.kind %}
              {% when HistoryEntryKind::Current %}Current - last edited {{ entry.edited_at }}
              {% when HistoryEntryKind::Prior %}Edited {{ entry.edited_at }}
            {% endmatch %}
          </div>
          <div class="markdown-body">{{ entry.body_html|safe }}</div>
        </div>
        {% endfor %}
      </div>
    </aside>
    ```

  `{{ entry.body_html|safe }}` is intentional: the body has already been escaped and sanitized by `views::markdown::render`. Same pattern that `message.html` uses for live bodies.

- [ ] In `server/templates/room/page.html` and `server/templates/dm/page.html`, add `<aside id="history-panel" class="hidden"></aside>` as a sibling of the existing `<aside id="thread-panel">`. Place it immediately after the thread-panel slot.
- [ ] In `server/templates/room/message.html`, replace the existing `(edited)` span:

    ```html
    {% if message.edited_at.is_some() %}
    <button
      hx-get="/messages/{{ message.id }}/history"
      hx-target="#history-panel"
      hx-swap="outerHTML"
      class="text-xs text-slate-400 hover:text-slate-600 hover:underline"
      aria-label="View edit history"
    >(edited)</button>
    {% endif %}
    ```

  Per phase 25 accessibility: the existing `:focus-visible` global rule covers focus indication; the `aria-label` is the explicit accessible name. No new listeners, so phase 20's listener-cleanup discipline is not invoked.

- [ ] `./dev/cargo check -p lets-chat-server` (Askama compiles templates at build time) clean.
- [ ] `just build-css` if Tailwind utilities new to the file fail to appear during dev.
- [ ] `git add` the touched template files and stop.

### Task 6 - Tests

- [ ] Extend `server/tests/message_editing.rs`:
    - Edit once: `SELECT count(*) FROM message_edits WHERE message_id = ?` is 1; the row's `previous_body` equals the body before the edit; the row's `edited_at` equals the new `messages.edited_at`.
    - Edit twice: 2 rows in chronological order by `(edited_at, id)`. Bodies match the displaced sequence.
    - The existing `MessageEdited` WS broadcast still fires (no contract change to step 5 of the handler flow).

- [ ] New `server/tests/routes_message_edit_history.rs`:
    - Unedited message: `GET /messages/:id/history` returns a fragment containing exactly one entry, the "Current" version with the message's body.
    - After two edits: fragment contains three entries in order (prior, prior, current). Assert on the rendered HTML containing the right bodies in the right slots.
    - Unauthenticated request: same redirect/401 shape as `get_single_message` for unauthenticated callers. Assert against the actual existing behavior (do not assume; mirror the assertion shape used in `routes_messages.rs` or whichever existing file tests similar gates).
    - Non-room-member: receives the same status as `get_single_message` for the same caller/message - whichever of `Forbidden` (403) or `NotFound` (404) the combined `get_message` + `is_room_accessible` produces.
    - Soft-deleted message: returns 404. `get_message` filters `deleted_at IS NULL` so this falls out for free.
    - Markdown rendering of a prior body: insert a row with `**bold**` as `previous_body`, hit the endpoint, assert response contains `<strong>bold</strong>`.
    - Mention chip in a prior body: insert a row with `@<username>` referencing an existing user, hit the endpoint, assert the chip HTML for that user is present. This exercises the resolve-against-current-usernames behavior.
    - Drift-trap line: include `include_str!("../migrations/chat/0025_message_edits.sql")` in the hand-rolled pool setup if the file does not use `common::chat_pool()`. (Test-binary author decides; prefer `common::chat_pool()` for new files since phase 24's helper picks up migrations automatically.)

- [ ] `just test` clean.
- [ ] `just test-saas` clean. The new route is not feature-gated (no `#[cfg(feature = "...")]` on the route registration or the handler), so the same tests run in both modes. Confirm by reading the new code before declaring done.
- [ ] `git add server/tests/message_editing.rs server/tests/routes_message_edit_history.rs` and stop.

### Task 7 - Manual verification

- [ ] `just dev-web-local` to bring up the local stack at `http://localhost:18080`.
- [ ] Send a message in `#general`. Edit it. Edit it again. Click the `(edited)` button.
- [ ] Drawer opens on the right next to the message list. Three entries: two prior + current, oldest-first. Markdown renders identically to the live message. Close button (`&#10005;`) closes the drawer.
- [ ] Open a thread on a different message (existing thread feature). Confirm the thread panel is still functional. Now click `(edited)` on an edited message: both `<aside>` panels coexist as siblings, neither replaces the other. Layout may be cramped on narrow widths; acceptable for v1 per §Out of scope.
- [ ] Soft-delete a message via the existing moderation UI (`Mod` user clicks delete on someone else's message). Confirm the `(edited)` button no longer appears for that message (because it is filtered out of the room view by `deleted_at IS NULL`). Hitting `/messages/:id/history` directly returns 404; confirm by URL bar or curl.
- [ ] Edit a thread reply. Click `(edited)` on the reply inside the thread panel. The history drawer opens for that reply specifically. Both panels open simultaneously, as siblings.
- [ ] `git status` shows only intentional changes.

### Task 8 - Push branch, open PR

- [ ] `just check` passes (server + desktop + clippy + fmt).
- [ ] Commits on this branch (`plan/phase26-edit-history` if you started from the plan branch, else `feat/phase26-edit-history`) are clean.
- [ ] `fj pr create "feat(messages): record prior bodies on edit; expose history drawer" --body-file <(mktemp --tmpdir --suffix .md)`. The PR body restates the §Out of scope list verbatim so reviewers know what is and is not in this phase. Single line per bullet (no hard-wrapping inside bullets - the Forgejo UI wraps).

## Forward-compat notes

Captured here so the next person reading this plan when admin-edit lands does not have to reconstruct the decision:

- **Adding `editor_user_id`.** Migration is the `ALTER TABLE` + backfill in §Schema. The DB function signature becomes `update_message_body(pool, message_id, new_body, editor_user_id: &str)`. The single call site at `routes/room.rs:742` passes `user.id` once admin/mod editing exists; until then the column is hardcoded to the message author by the backfill.
- **Mod-audit access to soft-deleted history.** Do NOT widen `get_message` to "include deleted." Add a separate `get_message_for_moderation` query and a dedicated `/admin/audit/messages/:id/history` route, gated on `AdminUser`. Keeps the user-facing gate semantics narrow and avoids accidentally exposing deleted content to room members.
- **Push proactive history-panel updates.** If a future product requirement says "the drawer should stay live while open," the existing `MessageEdited` event already carries enough information; route a per-message subscriber list through the hub and re-fetch the panel client-side. Today's static-after-open shape is intentional.
- **Diff view.** When this lands, the right entry point is `HistoryEntryView` gaining a `diff_html: Option<String>` field rendered alongside `body_html`. The renderer would be a separate function in `views::markdown` (or a sibling module) that takes two bodies and emits unified diff HTML. Not in this phase.

---

This is one PR. Tasks 1-7 are commits inside it. Tasks 6 and 7 are gating: do not open the PR until both `just test` / `just test-saas` are green AND the manual smoke covers the cases in Task 7.
