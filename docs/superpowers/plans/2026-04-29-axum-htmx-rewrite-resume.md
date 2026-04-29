# Axum+HTMX Rewrite - Resume Notes

Resume point for continuing the rewrite from a fresh Claude Code session.

## Branch

`feat/axum-htmx-rewrite` (pushed). Current HEAD: `38fa3ed refactor(views): introduce Html wrapper helper; drop dead asset_url`.

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

`./dev/cargo`, `./dev/bun`, `./dev/server-up`, `./dev/server-down`, `./dev/server-logs` — all wrap Docker. The cargo wrapper uses `rust:1.88-slim-bookworm` (was bumped from 1.83 by Task 2 because transitive deps require edition2024). `./dev/server-up` publishes container port 8080. The dev host already binds 0.0.0.0:8080 (cadvisor); always pass `HOST_PORT=18080` when starting:

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

## Plan-wide conventions to read first

`docs/superpowers/plans/2026-04-29-axum-htmx-rewrite.md`, lines 152-180. Two substitutions are in force globally:

1. **No em-dashes.** Anywhere the plan shows `—` in a code block (template title strings, etc.), substitute `-` when copying into the codebase.

2. **`askama_axum` is NOT used.** The codebase has `server/src/views/mod.rs::Html(pub String)` and `views::html(&template) -> Result<Html, AppError>`. Every plan code block that imports `use askama_axum::Template;` and ends a handler with `Ok(page.into_response())` should be transformed to:
   - `use askama::Template;` (still needed for the derive)
   - Handler return type `Result<Html, AppError>`
   - Handler body ends with `crate::views::html(&page)` or `Ok(html(&page)?)`
   - For ad-hoc inline HTML responses like `axum::response::Html(html_string).into_response()` (used for tiny inline-built fragments such as the reaction picker in Task 11), **keep** `axum::response::Html(...)` — that's a different type and works as-is.

## Status

| Task | Status | Notes |
|------|--------|-------|
| 1 | DONE (commit `d7ddac1` + fix `7ec92f6`) | Workspace conversion + vendor htmx assets |
| 2 | DONE (commit `6d4f4a0`) | Strip Dioxus, empty Axum server. Latent dual-Hub bug noted in plan Task 8 pre-step |
| 3 | DONE (commits `f313cea` + refactor `38fa3ed`) | Base layout + welcome page. Html wrapper helper introduced |
| 4 | NOT STARTED | Cookie auth middleware + AuthUser/AdminUser extractors. Implementer dispatch was prepared (see "Resume Task 4" below) but rejected to free session budget |
| 5-17 | NOT STARTED | |

Three commits beyond the plan's prescribed flow:
- `db74bc1` — Docker cargo/bun wrappers under `dev/`
- `7111716` — plan refinement: note Hub-instance unification as Task 8 pre-step
- `38fa3ed` — Html helper + plan-wide convention note

## Resume Task 4 - implementer prompt

Dispatch a fresh subagent with this prompt (substitute `<HEAD-SHA>` with whatever `git log` shows is current, currently `38fa3ed`):

```
You are implementing Task 4 of the lets-chat Axum+HTMX rewrite.

Working directory: /home/nate/.config/superpowers/worktrees/lets-chat/feat-axum-htmx-rewrite (or wherever the feat/axum-htmx-rewrite branch lives on this machine).
Current HEAD: <HEAD-SHA>.

Critical environment: NO Rust or Bun on host. Use ./dev/cargo and ./dev/server-up; HOST_PORT=18080 must be passed to ./dev/server-up.

Read the plan-wide conventions section (lines 152-180 of docs/superpowers/plans/2026-04-29-axum-htmx-rewrite.md). Two substitutions:
- No em-dashes in any code or template; use hyphens.
- Use `crate::views::{html, Html}` helper instead of `askama_axum::Template`. Handler returns `Result<Html, AppError>` and ends with `Ok(html(&page)?)`.

Implement Task 4 from the plan (search "## Task 4: Cookie auth middleware"). Adjustments:

- Step 2 (auth.rs): use `#[async_trait::async_trait]` (the `async-trait` crate is already a dep), not `#[axum::async_trait]` — axum 0.8 deprecated the latter.
- Step 4: routes/home.rs becomes the `Html` handler shown below.
- Step 5: confirm `grep -rn 'User::placeholder' server/src` returns empty after deletion.
- Step 6: `HOST_PORT=18080 ./dev/server-up -p lets-chat-server`, then verify `curl --silent --include http://127.0.0.1:18080/ | head -3` shows `303 See Other` with `location: /login`. Stop with ./dev/server-down.

Verify the actual User struct first: `cat server/src/models/user.rs`. The role field is a `String`, not a `UserRole` enum. AdminUser must do `match user.role.as_str() { "admin" => ..., _ => ... }` accordingly.

Acceptance:
- server/src/auth.rs has inject_user + AuthUser + OptionalUser + AdminUser + SESSION_COOKIE constant.
- routes/mod.rs adds `.layer(middleware::from_fn_with_state(state.clone(), inject_user))`.
- routes/home.rs uses AuthUser; User::placeholder is removed.
- ./dev/cargo check -p lets-chat-server passes.
- GET / without cookie returns 303 to /login.
- One new commit subject starting `feat(auth): add cookie middleware and AuthUser/AdminUser extractors`.

Updated home.rs to write:
```rust
use axum::extract::State;
use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::home::WelcomePage;
use crate::views::{html, Html};

pub async fn get_home(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Html, AppError> {
    let page = WelcomePage { user: &user, asset_version: state.asset_version };
    html(&page)
}
```

Self-review then report (DONE / DONE_WITH_CONCERNS / BLOCKED / NEEDS_CONTEXT).
```

Then dispatch a spec-compliance subagent (template at `~/.claude/plugins/cache/claude-plugins-official/superpowers/5.0.7/skills/subagent-driven-development/spec-reviewer-prompt.md`), then a `superpowers:code-reviewer` subagent (template at `code-quality-reviewer-prompt.md` in the same dir), then mark Task 4 done and move to Task 5.

## Resume Tasks 5-17

For each remaining task, follow the same flow:

1. Pre-read the plan section.
2. Apply the global conventions (no em-dashes, use `views::html`/`Html`, use `./dev/cargo` and `./dev/server-up`).
3. Dispatch implementer (`general-purpose` subagent).
4. Dispatch spec-compliance reviewer.
5. Dispatch code-quality reviewer (`superpowers:code-reviewer`).
6. Apply fixes, mark TaskUpdate complete, move to next.

Tasks remaining (per the plan):
- Task 4: Cookie auth middleware + AuthUser/AdminUser extractors
- Task 5: Login, register, logout
- Task 6: Sidebar with rooms list and DM list
- Task 7: Room view (read-only) GET /room/:id
- Task 8: WebSocket route with HTML fragment broadcast (do the Hub-instance unification pre-step from the plan)
- Task 9: Send messages POST /room/:id/messages
- Task 10: Edit + delete messages
- Task 11: Reactions
- Task 12: Direct messages /dm/:user_id
- Task 13: Search /search
- Task 14: Admin pages
- Task 15: Read receipts + unread badges
- Task 16: Desktop wrapper Tao+Wry
- Task 17: Justfile + Docker + cleanup + merge

## Watchpoints discovered so far

1. **`db` API names referenced in plan may not exist verbatim.** The plan references helpers like `list_rooms_visible_to`, `list_dm_peers`, `recent_messages`, `reaction_counts_for`, `get_or_create_dm_room`, `set_last_read`, `unread_count`. Tasks 6, 7, 11, 12, 15 each say "verify before implementing" — when subagents hit a missing helper, lift the SQL from the deleted server_fns via `git show <pre-rewrite-sha>:server/src/server_fns/<file>.rs` (e.g., `git show 4a483d3:src/server_fns/rooms.rs`).

2. **User struct.** Fields are: `id, username, display_name, role (String), is_muted, muted_until, is_banned, ban_reason, banned_until, created_at, read_receipts_enabled`. No `email`, no `UserRole` enum. Plan code samples that show `UserRole::Admin` need conversion to string match.

3. **Hub-instance unification.** Task 8 must delete `static HUB: OnceLock<Arc<Hub>>` and `get_hub()` from `server/src/ws/hub.rs`, and refactor `notify_typing` to take `self: &Arc<Self>`. The plan's pre-step section (lines 1816-1828 of the plan) covers this.

4. **`db::auth::register_user` return type.** Last commit on main (`95dcc1c`) made it return `Result<User, RegisterError>` with a `UsernameTaken` variant. Task 5's plan code assumes that shape.

5. **Cookie name.** `session` (lower-case). Defined in plan + ws/handler.rs. Reused by `auth.rs::SESSION_COOKIE`.

6. **`HOST_PORT=18080` always.** 8080 conflicts with cadvisor on the dev host.

7. **`tailwind.config.js` content glob.** Must include `./templates/**/*.html` (Task 3 fixed this). Don't regress.

8. **`bun.lock` is committed.** Future `bun install` invocations should pass `--frozen-lockfile`.

9. **Tests untouched so far.** Existing `server/tests/db_*.rs` tests still compile (they only use `lets_chat::db::*` helpers). Don't break them. New handler-level tests (`tests/handler_*.rs`) land in respective tasks.

## Context the next session should grab on entry

- `git log --oneline main..HEAD` — see all rewrite commits.
- `cat docs/superpowers/plans/2026-04-29-axum-htmx-rewrite.md` (lines 1-200) for spec + conventions.
- `cat docs/superpowers/plans/2026-04-29-axum-htmx-rewrite-resume.md` (this file).
- `ls dev/` — confirm wrappers present.
- `./dev/cargo check -p lets-chat-server` — should still pass at HEAD.

If `./dev/cargo check` fails on resume, that's the first thing to fix before continuing.
