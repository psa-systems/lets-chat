# Enclaves — Phase 4: Real-time + Cleanup

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Prereq:** Phases 1-3 merged onto `feat/enclaves`.

**Goal:** Wire up live updates over WebSocket for enclave membership, room set, and invitation events; remove the legacy `POST /admin/rooms`; final integration smoke + PR.

**Architecture:** New `ChatEvent` variants and per-event hub broadcasts that target the affected user(s). Each handler renders the relevant OOB fragment (switcher or sidebar) so the recipient's UI updates without a refresh.

**Tech Stack:** Axum WebSocket, HTMX `hx-swap-oob`, existing Hub (`DashMap<RoomId, Vec<UnboundedSender<...>>>` plus `broadcast_to_user`).

---

## File Structure (Phase 4)

| File | Purpose |
|---|---|
| `server/src/ws/events.rs` | New `ChatEvent` variants. |
| `server/src/ws/handler.rs` (or wherever the WS recv loop lives) | OOB fragment rendering for new events. |
| `server/src/routes/enclave.rs` | Trigger new broadcasts after each mutating handler. |
| `server/src/routes/admin.rs` | Remove `POST /admin/rooms`; keep grouped GET. |
| `server/templates/ws/enclave_switcher_oob.html` | OOB fragment template (re-renders the switcher). |
| `server/templates/ws/enclave_sidebar_oob.html` | OOB fragment template (re-renders the per-enclave sidebar). |
| `server/tests/ws_enclave_events.rs` | Integration coverage for the new events. |

---

## Task 1: Extend `ChatEvent`

**Files:**
- Modify: `server/src/ws/events.rs`
- Test: `server/tests/ws_enclave_events.rs`

- [ ] **Step 1: Add variants**

Append to the `ChatEvent` enum:

```rust
EnclaveMemberAdded { enclave_id: i64, user_id: String },
EnclaveMemberRemoved { enclave_id: i64, user_id: String },
EnclaveRoomAdded { enclave_id: i64, room_id: i64 },
EnclaveRoomRemoved { enclave_id: i64, room_id: i64 },
EnclaveInvitationCreated { invitee_id: String },
EnclaveInvitationResolved { invitee_id: String },
```

- [ ] **Step 2: Compile-only verify**

Run: `./dev/cargo build -p lets-chat-server`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add server/src/ws/events.rs
git commit -m "feat(enclaves): add ChatEvent variants for membership, rooms, invitations

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: OOB fragment templates

**Files:**
- Create: `server/templates/ws/enclave_switcher_oob.html`
- Create: `server/templates/ws/enclave_sidebar_oob.html`

- [ ] **Step 1: Switcher OOB**

```html
<!-- server/templates/ws/enclave_switcher_oob.html -->
{% include "partials/enclave_switcher.html" %}
```

The included partial already starts with `<nav id="switcher" ...>`; the recipient's HTMX handler swaps it via `hx-swap-oob="outerHTML"`. Wrap it explicitly if the partial doesn't already opt in:

```html
<nav id="switcher" hx-swap-oob="outerHTML" class="...">
  ...existing partial body...
</nav>
```

(Adjust `partials/enclave_switcher.html` to accept an `oob` flag mirroring the existing sidebar OOB pattern.)

- [ ] **Step 2: Sidebar OOB**

Same idea for `enclave_sidebar_oob.html` wrapping `partials/enclave_sidebar.html`.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(enclaves): OOB fragment templates for switcher + sidebar

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: WS handler renders new events

**Files:**
- Modify: the WebSocket recv loop where `ChatEvent` is matched (likely `server/src/routes/ws.rs` or `server/src/ws/handler.rs`).

- [ ] **Step 1: Match the new variants**

For each new event, render and send:

- `EnclaveMemberAdded { user_id }` and `EnclaveMemberRemoved { user_id }`: when the connection's `user_id` matches the event's `user_id`, rebuild the user's `ChromeView` (use the same handler-side `load_chrome` call but with the connection's `current_enclave`) and render `ws/enclave_switcher_oob.html`. Send the rendered HTML over the socket.
- `EnclaveRoomAdded { enclave_id, .. }` / `EnclaveRoomRemoved`: when the connection currently has `current_enclave == Some(enclave_id)`, rebuild and send `ws/enclave_sidebar_oob.html`.
- `EnclaveInvitationCreated { invitee_id }` / `EnclaveInvitationResolved { invitee_id }`: when connection's user matches, send a switcher refresh (Home icon badge).

The connection's `current_enclave` lives in the per-socket state; the existing typing/subscribe logic shows how to read per-connection context.

- [ ] **Step 2: Run `just check` + integration tests**

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(enclaves): WS handler renders OOB fragments for enclave events

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Trigger broadcasts from enclave handlers

**Files:**
- Modify: `server/src/routes/enclave.rs`

For each mutating handler, add a `state.hub.broadcast_to_user(...)` (or `broadcast_to_users(...)` for multi-target) call after the DB write succeeds. Specifically:

| Handler | Event | Recipient(s) |
|---|---|---|
| `post_create_room` | `EnclaveRoomAdded` | every member of the enclave |
| `post_delete_room` | `EnclaveRoomRemoved` | every member of the enclave |
| `post_invite` | `EnclaveInvitationCreated { invitee_id }` | invitee |
| `post_invitation_accept` | `EnclaveMemberAdded { user_id = caller }` and `EnclaveInvitationResolved { invitee_id }` | caller |
| `post_invitation_decline` | `EnclaveInvitationResolved { invitee_id }` | caller |
| `post_discover_join` | `EnclaveMemberAdded` | caller |
| `post_join_by_code` | `EnclaveMemberAdded` | caller |
| `post_kick` | `EnclaveMemberRemoved` | kicked user |
| `post_leave` | `EnclaveMemberRemoved` | caller |
| `post_member_role` | `EnclaveMemberAdded` (re-render) | target user |
| `post_transfer` | `EnclaveMemberAdded` (re-render) | both old and new owner |
| `post_delete` (delete enclave) | `EnclaveMemberRemoved` per former member | every former member (capture list before delete) |

- [ ] **Step 1: Wire each broadcast**

Inside each handler, after `db::enclave::*` returns Ok, iterate the recipient list and call `state.hub.broadcast_to_user(uid, &event)`.

For `post_delete`, capture `db::enclave::list_members(...)` before the cascade DELETE and broadcast to that list afterward.

- [ ] **Step 2: Run integration tests**

- [ ] **Step 3: Commit**

```bash
git add server/src/routes/enclave.rs
git commit -m "feat(enclaves): broadcast member/room/invitation events from handlers

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Remove `POST /admin/rooms`; keep grouped GET

**Files:**
- Modify: `server/src/routes/admin.rs`
- Modify: `server/templates/admin/rooms.html`

- [ ] **Step 1: Drop the route**

Remove the `.route("/admin/rooms", get(get_rooms).post(post_create_room))` POST half; site admins now create rooms via the per-enclave path (god-mode lets them pick any enclave on the discover or enclave landing page).

- [ ] **Step 2: Update template**

Remove the create form from `templates/admin/rooms.html`. The page becomes a read-only moderation listing grouped by enclave.

- [ ] **Step 3: Run all tests + check**

- [ ] **Step 4: Commit**

```bash
git add server/src/routes/admin.rs server/templates/admin/rooms.html
git commit -m "feat(enclaves): remove legacy POST /admin/rooms; admin listing read-only

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: End-to-end smoke + PR

**Files:** none.

- [ ] **Step 1: Full check**

Run: `just check`
Run: `./dev/cargo test -p lets-chat-server`
Run: `just verify`

- [ ] **Step 2: Manual two-tab test**

- Open `just dev-web-local` in two browser sessions, each logged in as a different user.
- User A creates an enclave and direct-invites User B.
- User B sees the badge tick up on Home in real time and sees the invitation appear at `/invitations` without refresh.
- User B accepts; the enclave icon appears in B's switcher live.
- User A adds a room; B sees the room in the sidebar live (when B has that enclave selected).
- A kicks B; B's switcher loses the icon live.

- [ ] **Step 3: Push + open PR**

```bash
git push -u origin feat/enclaves
gh pr create --title "feat: enclaves" --body "$(cat <<'EOF'
## Summary
- Adds enclaves: top-level groupings of rooms with three-tier roles (owner/admin/member) plus site-admin god-mode.
- Existing rooms migrate into a default General enclave; existing users become its members via a cross-DB startup backfill.
- New routes for enclave CRUD, invitations, public discovery, room ops, and member management.
- Two-column chrome (Discord-style switcher + per-enclave sidebar) replaces the single sidebar.
- `last_visited` cookie redirect on `/`; search now scoped to the current enclave or DMs.
- WebSocket events update switcher/sidebar live for membership, room, and invitation changes.

Spec: `docs/superpowers/specs/2026-05-05-enclaves-design.md`
Plan: `docs/superpowers/plans/2026-05-05-enclaves-master.md` and the four phase plans.

## Test plan
- [x] `just check`
- [x] `just verify`
- [x] `./dev/cargo test -p lets-chat-server` (all suites)
- [x] Two-browser manual smoke covering create/invite/accept/discover/transfer/delete/kick + live updates
EOF
)"
```

- [ ] **Step 4: Switch back to main**

After the PR merges, run `git checkout main && git pull` per the project convention.

---

## Phase 4 Done — feature shipped.
