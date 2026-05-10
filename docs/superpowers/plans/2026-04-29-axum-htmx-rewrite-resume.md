# Axum+HTMX Rewrite - Resume Notes

Resume point for continuing the rewrite from a fresh Claude Code session.

## Branch

`feat/axum-htmx-rewrite`. Current HEAD: `b830011 feat(search): full-text message search via /search?q=...`.

The spec and plan live on `spec/axum-htmx-rewrite` (already pushed; PR open: https://dev.a8n.run/a8n-tools/lets-chat/compare/main...spec/axum-htmx-rewrite). The feat branch was forked from spec, so it carries the spec doc, the plan doc, and all infra commits.

## Worktree

`/home/nate/.config/superpowers/worktrees/lets-chat/feat-axum-htmx-rewrite` on the original machine. On a fresh machine, recreate with:

```bash
git fetch origin feat/axum-htmx-rewrite
git worktree add ~/.config/superpowers/worktrees/lets-chat/feat-axum-htmx-rewrite feat/axum-htmx-rewrite
cd ~/.config/superpowers/worktrees/lets-chat/feat-axum-htmx-rewrite
```

Or skip worktrees and just checkout the branch in your normal clone.

## Toolchain (no Rust on host)

`./dev/cargo`, `./dev/bun`, `./dev/server-up`, `./dev/server-down`, `./dev/server-logs` — all wrap Docker. The cargo wrapper uses `rust:1.88-slim-bookworm`. `./dev/server-up` publishes container port 8080. The dev host already binds 0.0.0.0:8080 (cadvisor); always pass `HOST_PORT=18080` when starting:

```bash
HOST_PORT=18080 ./dev/server-up -p lets-chat-server
```

Named Docker volumes (`lets-chat-rewrite-cargo-registry`, `-cargo-git`, `-target`, `-data`, `-bun-cache`) cache state across runs. On a brand-new host, the first run of `./dev/cargo` may need a one-time chown:

```bash
docker run --rm -v lets-chat-rewrite-cargo-registry:/d rust:1.88-slim-bookworm chown -R "$(id -u):$(id -g)" /d
docker run --rm -v lets-chat-rewrite-cargo-git:/d rust:1.88-slim-bookworm chown -R "$(id -u):$(id -g)" /d
docker run --rm -v lets-chat-rewrite-target:/d rust:1.88-slim-bookworm chown -R "$(id -u):$(id -g)" /d
docker run --rm -v lets-chat-rewrite-data:/d rust:1.88-slim-bookworm chown -R "$(id -u):$(id -g)" /d
```

## Plan-wide conventions (in plan file lines 152-180)

1. **No em-dashes.** Substitute `-` for `—` everywhere in code, templates, commits, docs.
2. **`askama_axum` is NOT used.** Use the in-house helper at `server/src/views/mod.rs`:
   - `pub struct Html(pub String)` plus `impl IntoResponse`
   - `pub fn html<T: askama::Template>(t: &T) -> Result<Html, AppError>`
   - Handler return type `Result<Html, AppError>`; body ends with `html(&page)` or `Ok(html(&page)?)`
   - For ad-hoc inline HTML, keep `axum::response::Html(string)` as-is (different type).
3. **Use the `From<UserRecord> for User` impl** at `server/src/models/user.rs` (introduced commit `bd1edb9`) — call `record.into()` rather than reconstructing field-by-field.

## Status

| Task | Status | Commit | Notes |
|------|--------|--------|-------|
| 1 | DONE | `d7ddac1` + fix `7ec92f6` | Workspace conversion + vendor htmx assets |
| 2 | DONE | `6d4f4a0` | Strip Dioxus, empty Axum server |
| 3 | DONE | `f313cea` + refactor `38fa3ed` | Base layout + welcome page; Html wrapper helper |
| 4 | DONE | `2c8375d` | Cookie auth middleware + extractors |
| 5 | DONE | `2f4b5d5` + fix `512b109` | Login, register, logout (transactional first-user-admin) |
| 6 | DONE | `522ddc2` + refactor `bd1edb9` | Sidebar with rooms + DM peers |
| 7 | DONE | `08b5d99` | Room view (read-only) |
| 8 | DONE | `5061d23` + fix `b72e7f1` | WebSocket fragment broadcast (Hub unified) |
| 9 | DONE | `eb1ea0a` | Send message POST /room/:id/messages |
| 10 | DONE | `0741d24` | Edit + delete message endpoints |
| 11 | DONE | `b9b95fe` | Reactions with HTMX picker |
| 12 | DONE | `c79910a` | Direct messages /dm/:peer_id |
| 13 | DONE | `b830011` | Full-text search /search?q=... |
| 14 | NOT STARTED | — | Admin pages |
| 15 | NOT STARTED | — | Read receipts + unread badges |
| 16 | NOT STARTED | — | Desktop wrapper Tao+Wry |
| 17 | NOT STARTED | — | Justfile + Docker + cleanup + merge |

Three commits beyond the plan's prescribed flow:
- `db74bc1` — Docker cargo/bun wrappers under `dev/`
- `7111716` — plan refinement: note Hub-instance unification as Task 8 pre-step
- `38fa3ed` — Html helper + plan-wide convention note

## Resume Task 14 (next) - implementer prompt

Dispatch a fresh subagent (general-purpose) with this prompt. Update HEAD-SHA to current `git log --oneline -1`.

```
You are implementing Task 14 of the lets-chat Axum+HTMX rewrite. Admin pages: settings, users, invites, rooms, mod log.

Working directory: /home/nate/.config/superpowers/worktrees/lets-chat/feat-axum-htmx-rewrite (branch feat/axum-htmx-rewrite, current HEAD b830011).

Critical environment: NO Rust or Bun on host. Use ./dev/cargo and ./dev/server-up; HOST_PORT=18080 must be passed to ./dev/server-up.

Read plan-wide conventions in docs/superpowers/plans/2026-04-29-axum-htmx-rewrite.md lines 152-180. No em-dashes; use crate::views::{html, Html}; use record.into() for UserRecord -> User.

Pre-reads:
- server/src/auth.rs (AdminUser extractor exists)
- grep -n 'pub async fn|pub fn' server/src/db/auth.rs (list_users, set_user_role, ban/unban)
- grep -n 'pub async fn|pub fn' server/src/db/chat.rs (admin room helpers)
- grep -n 'pub async fn|pub fn' server/src/db/moderation.rs (invite + mod log)
- grep -n 'pub async fn|pub fn' server/src/db/settings.rs
- cat server/src/models/{invite,mod_action,settings}.rs

If a needed helper doesn't exist as a 5-line wrapper, recover the SQL from git history: git show 4a483d3:src/server_fns/admin.rs and git show 4a483d3:src/components/admin/{users,invites,rooms,mod_log,settings}.rs.

Implement Task 14 from the plan (## Task 14: Admin pages). Seven sub-steps; some are open-ended ("write each remaining template fully") - follow plan's pattern.

Adaptations:
- Each admin page struct carries the four sidebar fields (user, rooms, dm_peers, asset_version) plus section: &'static str + page-specific data.
- All admin handlers gated on AdminUser extractor.
- Expose admin routes via pub fn router() -> Router<AppState> in routes/admin.rs.
- Ban broadcasts ChatEvent::UserBanned via state.hub.broadcast_global - confirm method name first.
- Use existing db helpers; only add 5-line wrappers if missing.

Verify: cargo check passes; admin user gets 200 on /admin, /admin/users, /admin/invites, /admin/rooms, /admin/modlog; non-admin gets 403. Sample smoke commands in the plan.

Caveat for smoke test: a previously-promoted admin from earlier test runs is in the persisted volume. Newly registered users won't be admin. Either clear the data volume or promote the new user via SQL:

  docker run --rm -v lets-chat-rewrite-data:/data alpine sh -c "apk add sqlite >/dev/null && sqlite3 /data/auth.db \"update users set role='admin' where username='$USR'\""

Commit subject must start: feat(admin): port admin pages to Askama+HTMX
```

After implementer reports DONE, dispatch combined spec+quality review (superpowers:code-reviewer with full diff context). Apply important fixes directly. Mark complete via TaskUpdate, move to Task 15.

## Resume Tasks 15-17

For each, follow the same flow:
1. Pre-read the plan section.
2. Apply global conventions.
3. Dispatch implementer (general-purpose subagent).
4. Dispatch combined spec+quality reviewer (superpowers:code-reviewer agent type).
5. Apply fixes, mark TaskUpdate complete, move to next.

Tasks remaining:
- Task 14: Admin pages
- Task 15: Read receipts + unread badges (per-room/DM unread counts in sidebar; mark-as-read on render)
- Task 16: Desktop wrapper Tao+Wry (~50 LOC native window pointing at LETS_CHAT_SERVER_URL)
- Task 17: Justfile + Docker + cleanup + merge (final infra rewrite + delete obsolete plans + update CLAUDE.md + open PR)

## Watchpoints discovered so far

1. **`db` API names differ from plan.** Real names found in this branch:
   - `db::chat::list_rooms(pool, user_id, is_admin)` (plan: list_rooms_visible_to)
   - `db::chat::list_user_dm_rooms(pool, user_id) -> Vec<(Room, peer_id String)>` (plan: list_dm_peers)
   - `db::chat::list_messages(pool, room_id, limit, before_id)` (plan: recent_messages)
   - `db::chat::list_reactions(pool, message_id, caller_user_id) -> Vec<Reaction>` with `reacted_by_me` field (plan: reaction_counts_for)
   - `db::chat::list_room_reactions(pool, room_id, caller_user_id) -> Vec<(message_id, Reaction)>`
   - `db::chat::toggle_reaction(pool, message_id, user_id, emoji) -> bool` (true=added, false=removed)
   - `db::chat::find_dm_room(pool, user_a, user_b)` + `db::chat::create_dm_room(pool, name, user_a, user_b)` (plan: get_or_create_dm_room — split into two)
   - `db::chat::is_room_member(pool, room_id, user_id)`
   - `db::chat::insert_message(pool, room_id, user_id, body) -> i64`
   - `db::chat::get_message(pool, id) -> Option<RawMessage>` (soft-deleted rows return None)
   - `db::chat::update_message_body(pool, id, body) -> String` (returns edited_at "%Y-%m-%d %H:%M:%S")
   - `db::moderation::soft_delete_message(pool, id, deleted_by)` (lives in moderation.rs not chat.rs!)
   - `db::chat::search_messages(pool, fts_query, room_id_filter, caller_user_id, is_admin) -> Vec<SearchResult>` (hardcoded LIMIT 50)
   - `db::chat::sanitize_fts_query(raw) -> Option<String>` (must call before search to escape FTS5 operators)
   - `db::auth::find_user_by_id`, `db::auth::find_user_by_username`, `db::auth::create_user(pool, username, password_hash) -> id`
   - `db::auth::create_session`, `delete_session`, `count_users`, `set_user_role`

2. **User struct.** Fields: `id, username, display_name, role (String), is_muted, muted_until, is_banned, ban_reason, banned_until, created_at, read_receipts_enabled`. No `email`, no `UserRole` enum. Use string match: `user.role == "admin"`.

3. **`Message`/`RawMessage`.** Use `user_id` not `author_id`. `created_at` is `String` ("%Y-%m-%d %H:%M:%S"). `Message` has `author_name` not `author_username`.

4. **`Room`.** No `is_private` field; use `room_type == "private"` (or "dm").

5. **`Reaction`.** Has `reacted_by_me` field; map to `ReactionView.viewer_reacted` at the boundary.

6. **`UserRecord`.** Use `record.into()` to convert to `User` (From impl on the model).

7. **Hub broadcast methods on `state.hub`:**
   - `broadcast_to_room(room_id, &event)` — fan out to subscribers of one room
   - `broadcast_global(&event)` — fan out to all connected users
   - `broadcast_to_user(user_id, &event)` — single user (all tabs)
   - `notify_typing(self: &Arc<Self>, conn_id, room_id)` — debounced typing presence (Task 8 unified the Hub instance)
   - `connect(user_id, username)`, `disconnect(conn_id)`, `subscribe(conn_id, room_id)`, `unsubscribe(conn_id, room_id)`

8. **WS reaction rendering.** `routes/ws.rs` handles `ChatEvent::ReactionAdded`/`ReactionRemoved` specially via `render_reaction_bar(&state, msg_id, &user_id)` (renders per-user with viewer_reacted state). All other events go through `views::ws_fragments::render_event(&event)`.

9. **WS subscribe wiring.** Use the public `htmx:wsOpen` event with `evt.detail.socketWrapper.send(...)`. Don't reach into `_htmxWebSocket` (private).

10. **`HOST_PORT=18080` always.** 8080 conflicts with cadvisor on the dev host.

11. **`tailwind.config.js` content glob.** Includes `./templates/**/*.html` and `./src/**/*.rs`. Don't regress.

12. **`bun.lock` is committed.** Future `bun install` invocations should pass `--frozen-lockfile`.

13. **Tests untouched so far.** Existing `server/tests/db_*.rs` tests still compile. Don't break them. New handler-level tests come later (none yet).

14. **Pre-existing data volume.** `lets-chat-rewrite-data` persists across runs. First-user-is-admin promotion only fires if `count_users == 1`. To promote a fresh user for admin testing, run SQL:
    ```
    docker run --rm -v lets-chat-rewrite-data:/data alpine sh -c "apk add sqlite >/dev/null && sqlite3 /data/auth.db \"update users set role='admin' where username='$USR'\""
    ```

15. **`db::moderation::soft_delete_message`** lives in `moderation.rs` not `chat.rs`. Surprised by this in Task 10.

## Context the next session should grab on entry

- `git log --oneline main..HEAD` — see all rewrite commits.
- `cat docs/superpowers/plans/2026-04-29-axum-htmx-rewrite.md` (lines 1-200) for spec + conventions.
- `cat docs/superpowers/plans/2026-04-29-axum-htmx-rewrite-resume.md` (this file).
- `ls dev/` — confirm wrappers present.
- `./dev/cargo check -p lets-chat-server` — should still pass at HEAD.

If `./dev/cargo check` fails on resume, that's the first thing to fix before continuing.
