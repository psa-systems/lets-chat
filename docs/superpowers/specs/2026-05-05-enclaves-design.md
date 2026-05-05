# Enclaves Design

## Summary

Introduce **enclaves**: top-level groupings of rooms (Discord-server / Slack-workspace analogue). Every non-DM room belongs to exactly one enclave. Users join enclaves and only then see that enclave's rooms. DMs remain site-wide and live on a "Home" pseudo-enclave alongside a welcome screen for new users.

## Goals

- Add enclaves as the unit users join to gain access to a set of rooms.
- Any user can create an enclave; the creator becomes its owner.
- Owners can promote members to admin, kick members, add/remove rooms, edit metadata, transfer ownership, delete the enclave.
- Owners and admins can invite users (direct invite or shareable code) and toggle the enclave's public-discovery flag.
- Existing public/private rooms migrate cleanly into a default "General" enclave; existing users become its members.
- Landing on `/` redirects to the user's last-visited room/DM if any, else shows the Home pseudo-enclave with the DM list and a welcome message.

## Non-Goals (v1)

- Per-enclave moderation beyond kick (no per-enclave ban/mute; the site-wide ban remains the only ban mechanism).
- Per-enclave notification preferences.
- Per-enclave audit log.
- Threaded conversations, voice, or video.
- Cross-enclave search (search is scoped to the current enclave).
- Custom enclave avatars/icons (initial-letter rendering only).
- Roles beyond `owner`/`admin`/`member`.
- Per-enclave invite codes with expiry/quota (one rotatable code per enclave).

## Terminology

- **Enclave**: a named container of rooms with its own member list and per-enclave roles.
- **Home**: a pseudo-enclave (no DB row) representing the DM hub. Selected when no real enclave is selected. URL: `/`.
- **General**: the real enclave created by the migration. Holds every pre-existing room.
- **Site admin**: existing top-level role (`users.role = 'admin'`). Granted god-mode over every enclave.

## Data Model

New chat-DB migration `server/migrations/chat/0009_enclaves.sql`:

```sql
CREATE TABLE enclaves (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    is_public   INTEGER NOT NULL DEFAULT 0,
    invite_code TEXT,
    created_by  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_enclaves_invite_code
    ON enclaves(invite_code) WHERE invite_code IS NOT NULL;

CREATE TABLE enclave_members (
    enclave_id  INTEGER NOT NULL REFERENCES enclaves(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('owner','admin','member')),
    joined_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (enclave_id, user_id)
);
CREATE UNIQUE INDEX idx_enclaves_one_owner
    ON enclave_members(enclave_id) WHERE role = 'owner';
CREATE INDEX idx_enclave_members_user ON enclave_members(user_id);

CREATE TABLE enclave_invitations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    enclave_id  INTEGER NOT NULL REFERENCES enclaves(id) ON DELETE CASCADE,
    invitee_id  TEXT NOT NULL,
    invited_by  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (enclave_id, invitee_id)
);
CREATE INDEX idx_enclave_invitations_invitee ON enclave_invitations(invitee_id);

ALTER TABLE rooms ADD COLUMN enclave_id INTEGER REFERENCES enclaves(id) ON DELETE CASCADE;
CREATE INDEX idx_rooms_enclave ON rooms(enclave_id);
```

### Migration data step (chat-DB SQL only)

The `0009_enclaves.sql` migration runs against `chat.db` and cannot read user records from `auth.db`. It performs the chat-side data move only:

1. Insert one row into `enclaves`: `name='General'`, `description='Default enclave'`, `created_by='system'` (sentinel — replaced during the startup backfill below).
2. `UPDATE rooms SET enclave_id = <general_id> WHERE room_type != 'dm';`
3. DM rows in `rooms` keep `enclave_id IS NULL` (DMs are cross-enclave).

### Startup backfill (cross-DB, idempotent)

After both pools migrate at server start, `main.rs` runs `db::enclave::backfill_general_membership(auth, chat)`. This function is idempotent and a no-op when the General enclave already has any members.

Behavior when `enclave_members` is empty and a `General` enclave exists:

- Read every user from `auth.users` ordered by `created_at ASC`.
- First site admin (lowest `created_at` with `role='admin'`) → insert as `enclave_members(role='owner')`.
- Remaining site admins → `role='admin'`.
- Everyone else → `role='member'`.
- If at least one site admin exists, `UPDATE enclaves SET created_by=<owner_id> WHERE name='General' AND created_by='system'`.
- If no site admin exists yet (fresh deploy with no users), the function does nothing; the next user to register will be auto-promoted to site admin (existing behavior) and then become the General owner via this same backfill on the *following* startup, or — better — `auth::register` calls `backfill_general_membership` after the first user's promotion completes.

### Invariants

- `rooms.enclave_id` is non-NULL when `room_type IN ('public','private')` and NULL when `room_type = 'dm'`. Enforced at the application layer (cross-column CHECK constraints in SQLite are awkward); covered by integration tests.
- Exactly one `role='owner'` per enclave (partial unique index `idx_enclaves_one_owner`).
- An enclave's owner is also an `enclave_members` row; "owner" is not a separate column on `enclaves`.
- Cascading deletes: deleting an enclave drops its rooms, members, and invitations. Deleting a room cascades to messages, `room_members`, and `dm_read_state` via existing FKs.

### Models (`server/src/models/enclave.rs`)

```rust
pub struct Enclave {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_public: bool,
    pub invite_code: Option<String>,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum EnclaveRole { Owner, Admin, Member }

pub struct EnclaveMembership {
    pub enclave_id: i64,
    pub user_id: String,
    pub role: EnclaveRole,
    pub joined_at: String,
}

pub struct EnclaveInvitation {
    pub id: i64,
    pub enclave_id: i64,
    pub invitee_id: String,
    pub invited_by: String,
    pub created_at: String,
}
```

`EnclaveRole` (de)serializes to/from the DB string column with explicit conversions; reject unknown strings as `sqlx::Error::Decode`.

## DB Module (`server/src/db/enclave.rs`)

- `create_enclave(pool, name, description, creator_id) -> Result<i64>` (transactional: row + owner membership).
- `get_enclave(pool, id) -> Result<Option<Enclave>>`.
- `get_enclave_by_invite_code(pool, code) -> Result<Option<Enclave>>`.
- `list_enclaves_for_user(pool, user_id) -> Result<Vec<Enclave>>`.
- `list_public_enclaves(pool) -> Result<Vec<Enclave>>`.
- `get_membership(pool, enclave_id, user_id) -> Result<Option<EnclaveMembership>>`.
- `list_members(pool, enclave_id) -> Result<Vec<EnclaveMembership>>` (caller resolves usernames against auth db, mirroring DM peer pattern).
- `add_member(pool, enclave_id, user_id, role)`.
- `remove_member(pool, enclave_id, user_id)`.
- `update_role(pool, enclave_id, user_id, role)`.
- `transfer_ownership(pool, enclave_id, new_owner_id)` (transactional: demote old owner to admin, promote new owner from admin/member, in a single tx so the partial unique index is never violated mid-flight).
- `delete_enclave(pool, id)`.
- `regenerate_invite_code(pool, enclave_id, new_code)` / `clear_invite_code(pool, enclave_id)`.
- `set_public(pool, enclave_id, is_public)`.
- `update_metadata(pool, enclave_id, name, description)`.
- `create_invitation(pool, enclave_id, invitee_id, invited_by)`.
- `list_invitations_for_user(pool, user_id) -> Result<Vec<(EnclaveInvitation, Enclave)>>`.
- `get_invitation(pool, id)`, `delete_invitation(pool, id)`.
- `accept_invitation(pool, id) -> Result<(i64, String)>` (transactional).

### Updates to `server/src/db/chat.rs`

- `create_room(pool, name, topic, room_type, invite_code, enclave_id: Option<i64>)` (DMs pass `None`).
- `list_rooms` replaced by `list_rooms_in_enclave(pool, enclave_id, user_id, can_see_all_private)`.
  - `can_see_all_private = true` when caller is enclave owner/admin or site admin.
- `list_room_unread_counts` gains `enclave_id: Option<i64>` filter (None = include DMs).
- New helper `is_room_accessible(pool, room_id, user_id, is_site_admin) -> Result<bool>` returns true if any of:
  - `is_site_admin`;
  - room is `dm` and user is in `room_members`;
  - user is in `enclave_members` for the room's enclave AND (`room_type='public'` OR user is in `room_members`).

## Permissions (`server/src/perms.rs`)

```rust
pub fn enclave_can_manage(role: Option<EnclaveRole>, site_role: &str) -> bool;
pub fn enclave_can_delete(role: Option<EnclaveRole>, site_role: &str) -> bool;
pub fn enclave_can_invite(role: Option<EnclaveRole>, site_role: &str) -> bool;
pub fn enclave_can_add_room(role: Option<EnclaveRole>, site_role: &str) -> bool;
pub fn enclave_can_manage_admins(role: Option<EnclaveRole>, site_role: &str) -> bool;
```

Each helper short-circuits to `true` when `site_role == "admin"` (god-mode). Otherwise the rules are:

| Capability                  | Owner | Admin | Member |
|-----------------------------|:-----:|:-----:|:------:|
| Read enclave rooms          |   X   |   X   |   X    |
| Send messages               |   X   |   X   |   X    |
| Edit enclave name/description | X | X |        |
| Toggle public visibility    |   X   |   X   |        |
| Generate / rotate invite code | X | X |        |
| Direct-invite a user        |   X   |   X   |        |
| Add / remove rooms          |   X   |   X   |        |
| Add / remove private-room members | X | X |        |
| Kick non-owner member       |   X   |   X   |        |
| Promote member ↔ admin      |   X   |       |        |
| Transfer ownership          |   X   |       |        |
| Delete enclave              |   X   |       |        |

## Routes

New module `server/src/routes/enclave.rs` mounted in `routes/mod.rs`.

```
POST   /enclaves                                       create enclave
GET    /enclave/{id}                                   enclave landing
POST   /enclave/{id}/edit                              update name/description
POST   /enclave/{id}/delete                            delete (owner)
POST   /enclave/{id}/transfer                          transfer ownership (owner)
POST   /enclave/{id}/visibility                        toggle is_public
POST   /enclave/{id}/invite-code                       generate / rotate code
DELETE /enclave/{id}/invite-code                       clear code
POST   /enclave/{id}/invite                            direct-invite username
POST   /enclave/{id}/members/{user_id}/role            promote / demote (owner)
POST   /enclave/{id}/members/{user_id}/kick            kick (owner|admin)
POST   /enclave/{id}/leave                             self-leave
POST   /enclave/{id}/rooms                             create room
POST   /enclave/{id}/rooms/{room_id}/edit              edit room
POST   /enclave/{id}/rooms/{room_id}/delete            delete room
POST   /enclave/{id}/rooms/{room_id}/members           add private-room member
POST   /enclave/{id}/rooms/{room_id}/members/{uid}/remove
GET    /enclaves/discover                              public-enclave list
POST   /enclaves/discover/{id}/join                    join public enclave
POST   /enclaves/join                                  join via invite code
GET    /invitations                                    pending invitations for caller
POST   /invitations/{id}/accept                        accept
POST   /invitations/{id}/decline                       decline
```

### Existing route changes

- `GET /` (`routes/home.rs`): read `last_visited` cookie. If present and the path resolves to an accessible room/DM, 302 there. Otherwise render the Home pseudo-enclave (DM list + welcome).
- `GET /room/{id}` (`routes/room.rs`): set `last_visited` cookie to `/room/{id}`. Resolve `enclave_id` from the room and use it for sidebar rendering.
- `GET /dm/{peer_id}` (`routes/dm.rs`): set `last_visited` cookie to `/dm/{peer_id}`.
- `GET /search` (`routes/search.rs`): scope is determined by query param `enclave_id`. When `enclave_id` is set and the caller is a member (or site admin), FTS is scoped to that enclave's rooms. When `enclave_id` is absent (Home), scope to DMs only. The previous unscoped `is_admin` global view is removed.
- `POST /admin/rooms` (`routes/admin.rs`): removed. Site admins create rooms inside an enclave via `/enclave/{id}/rooms` (god-mode permission allows this in any enclave). The `GET /admin/rooms` global listing is kept for moderation; it groups rooms by enclave name.

### Cookie

`last_visited` cookie:

- Name: `lets_chat_last_visited`.
- Value: URL-encoded path beginning with `/room/` or `/dm/`.
- Attributes: `HttpOnly`, `Secure`, `SameSite=Strict`, no explicit max-age (session cookie is fine; longevity is not load-bearing).
- Path validation on read: must match `^/room/\d+$` or `^/dm/[A-Za-z0-9-]+$`. Anything else is ignored. Inaccessible target → ignore + redirect to `/`.

## Sidebar / Layout

### Layout shift

`templates/layout.html` replaces the single `<aside id="sidebar">` with a flex container holding two siblings:

```html
<aside id="chrome" class="flex">
  {% include "partials/enclave_switcher.html" %}
  {% include "partials/enclave_sidebar.html" %}
</aside>
<main id="main">...</main>
```

Both partials accept an `oob` flag and, when set, render with `hx-swap-oob="outerHTML"` (matches the existing sidebar OOB pattern).

### Switcher partial (`templates/partials/enclave_switcher.html`)

- Narrow vertical column (~64px).
- Top: Home icon (`/`); badge counts total DM unread + pending-invitation count.
- One icon per enclave the caller is a member of (rendered as the first character of the enclave name); badge counts aggregate unread across rooms in that enclave.
- Bottom: "+" button → `/enclaves/discover` (a page that also exposes a "create enclave" form and a "join via code" form).
- The currently selected enclave (or Home) is visually highlighted.

### Sidebar partial (`templates/partials/enclave_sidebar.html`)

Two render modes driven by `current_enclave: Option<i64>`:

- **Home (`None`)**: DMs section (existing peer list) + welcome blurb above. Pending-invitation count surfaces at the top with a link to `/invitations`.
- **Inside an enclave (`Some(id)`)**: enclave name header + "Open rooms" section + "Private rooms" section (only rooms the caller belongs to) + a footer link "Settings" visible only to owner/admin/site-admin.

### Other templates

```
templates/enclave/page.html            landing: rooms list + members panel + invite UI
templates/enclave/settings.html        owner/admin management page
templates/enclave/members.html         member list with role badges + kick/promote
templates/enclave/member_row.html      single-member partial (HTMX swap target)
templates/enclave/room_row.html        single-room partial inside enclave landing
templates/enclave/discover.html        public-enclave list + create + join-by-code
templates/invitations/page.html        pending invitations
templates/invitations/row.html         single-invitation partial
```

`templates/home/welcome.html` updated copy: "Pick a DM, or create / join an enclave to chat in rooms."

`templates/partials/sidebar.html` deleted.

`templates/admin/rooms.html` simplified: site-admin global listing grouped by enclave name.

## WebSocket Events

Extend `ws::events::ChatEvent`:

```rust
EnclaveMemberAdded     { enclave_id: i64, user_id: String }
EnclaveMemberRemoved   { enclave_id: i64, user_id: String }
EnclaveRoomAdded       { enclave_id: i64, room_id: i64 }
EnclaveRoomRemoved     { enclave_id: i64, room_id: i64 }
EnclaveInvitationCreated  { invitee_id: String }
EnclaveInvitationResolved { invitee_id: String }
```

Hub broadcast targets:

- `EnclaveMemberAdded` / `Removed`: broadcast to the affected `user_id` only. Their handler re-renders the switcher (OOB) and clears stale enclave state.
- `EnclaveRoomAdded` / `Removed`: broadcast to every member of the enclave. Handler re-renders the enclave sidebar (OOB) only when the recipient currently has that enclave selected.
- `EnclaveInvitationCreated` / `Resolved`: broadcast to the invitee. Updates the Home badge + invitations page.

The existing `NewMessage` broadcast and unread-badge logic are preserved unchanged because `room_members` semantics are intact.

## Data Flow

### Login → first page

```
GET /
  read last_visited cookie
  if Some(path) and is_room_accessible / is_dm_accessible: 302 path
  else render Home pseudo (DM list + welcome)
```

### Create enclave

```
POST /enclaves { name, description }
  begin tx
    INSERT INTO enclaves (..., created_by = caller)
    INSERT INTO enclave_members (enclave_id, caller, role='owner')
  commit
  302 /enclave/{new_id}
```

### Direct invite + accept

```
POST /enclave/{id}/invite { username }
  guard: enclave_can_invite
  resolve username -> user_id (auth db)
  if user is already a member -> 400 with form re-render
  INSERT INTO enclave_invitations (UNIQUE collision -> idempotent: do not error, return same form state)
  hub.broadcast_to_user(invitee_id, EnclaveInvitationCreated)

POST /invitations/{id}/accept
  guard: invitation.invitee_id == caller
  begin tx
    INSERT INTO enclave_members (role='member')
    DELETE FROM enclave_invitations WHERE id = ?
  commit
  hub.broadcast_to_user(caller, EnclaveMemberAdded)
  302 /enclave/{enclave_id}
```

### Public-discovery join

```
POST /enclaves/discover/{id}/join
  guard: enclaves.is_public = 1 AND caller not already a member
  INSERT INTO enclave_members (role='member')
  hub.broadcast_to_user(caller, EnclaveMemberAdded)
  302 /enclave/{id}
```

### Add room to enclave

```
POST /enclave/{id}/rooms { name, topic, room_type }
  guard: enclave_can_add_room
  INSERT INTO rooms (enclave_id = id, ...)
  if room_type='private':
    INSERT INTO room_members (caller)
  hub.broadcast EnclaveRoomAdded to every member of the enclave
```

### Transfer ownership

```
POST /enclave/{id}/transfer { new_owner_id }
  guard: enclave_can_manage_admins (owner OR site admin)
  verify new_owner_id is a member
  begin tx
    UPDATE enclave_members SET role='admin' WHERE enclave_id=? AND role='owner'
    UPDATE enclave_members SET role='owner' WHERE enclave_id=? AND user_id=?
  commit
```

### Delete enclave

```
POST /enclave/{id}/delete
  guard: enclave_can_delete (owner OR site admin)
  capture member list before delete
  DELETE FROM enclaves WHERE id = ?    (cascades rooms, members, invitations; rooms cascade messages, room_members, dm_read_state)
  hub.broadcast EnclaveMemberRemoved to every former member
  302 /
```

## Error Handling

- Permission-guard failures → `AppError::Forbidden` (403).
- `enclaves.name` UNIQUE collision → form re-render with field error (matches register-flow pattern).
- `enclave_invitations` UNIQUE collision → silent success; do not surface "already invited" (avoids leaking membership state).
- `last_visited` cookie pointing to deleted/inaccessible target → ignore cookie, render Home, log nothing (this is normal).
- Owner attempts self-leave → `AppError::BadRequest("transfer ownership before leaving")`. The exception is when the owner is the *only* member of the enclave: in that case `/leave` is rejected and the user must use `/delete` instead (the form surfaces this explicitly).
- Owner attempts self-demote without transfer → same error; demote-self is only reachable through the transfer endpoint, which performs both updates atomically.
- Kick refuses to remove the owner role even under site-admin god-mode. To replace an owner, the site admin must call `/transfer` first (which atomically demotes the old owner to admin) and may then kick.
- Joining via invite code that no longer matches → form re-render with "invalid or revoked code".
- Room access guards (existing `room.rs`, `dm.rs`) now defer to `db::chat::is_room_accessible`, which encodes the enclave-membership rule.

## Testing

All tests use the existing in-memory SQLite pool harness in `server/tests/`.

### DB-layer tests

- `test_create_enclave_assigns_owner_role`
- `test_partial_unique_owner_index_prevents_two_owners`
- `test_transfer_ownership_atomic`
- `test_delete_enclave_cascades_rooms_members_invitations`
- `test_list_enclaves_for_user`
- `test_get_membership_returns_role`
- `test_invitation_unique_per_invitee`
- `test_is_room_accessible_admin_godmode`
- `test_is_room_accessible_enclave_member_open_room`
- `test_is_room_accessible_enclave_member_private_room_requires_room_member`
- `test_is_room_accessible_dm_unchanged`

### Integration tests

- `test_create_enclave_route`
- `test_invite_and_accept_flow`
- `test_join_via_invite_code`
- `test_public_discovery_lists_only_public_enclaves`
- `test_non_admin_cannot_delete_enclave` (403)
- `test_site_admin_can_manage_any_enclave` (god-mode)
- `test_room_visibility_inside_enclave` (non-member 403)
- `test_private_room_inside_enclave_requires_room_member`
- `test_dm_visibility_unchanged_cross_enclave`
- `test_search_scoped_to_current_enclave`
- `test_search_on_home_covers_dms_only`
- `test_last_visited_cookie_redirects_when_accessible`
- `test_last_visited_cookie_ignored_when_target_deleted`
- `test_owner_self_leave_rejected`
- `test_owner_transfer_then_leave_succeeds`
- `test_kick_owner_rejected_even_for_site_admin`
- `test_owner_alone_must_delete_not_leave`

### Migration tests

- `test_migration_0009_schema_changes`: seed an old-schema chat DB (rooms `general`, `random`, one private room with two members in `room_members`), apply migration `0009`, assert:
  - one `enclaves` row named `General` with `created_by='system'`,
  - all non-DM rooms have `enclave_id` set to it,
  - DM rows have `enclave_id IS NULL`,
  - existing `room_members` rows are intact,
  - `enclave_members` is empty (backfill is a separate step).
- `test_backfill_general_membership_assigns_roles`: seed auth DB with three users (admin, mod, regular) and a `General` enclave with `created_by='system'`, run `backfill_general_membership`, assert: admin is `owner`, mod is `member`, regular is `member`, and `enclaves.created_by` is the admin's user_id.
- `test_backfill_general_membership_idempotent`: running it twice yields the same `enclave_members` rows (no duplicates, no role flips).
- `test_backfill_skips_when_no_general_enclave`: backfill runs cleanly when General is missing (no panic).
- `test_backfill_skips_when_members_already_present`: backfill is a no-op when `enclave_members` has any row for General.

## Components

| File | Change |
|---|---|
| `server/migrations/chat/0009_enclaves.sql` | New: tables + columns + data migration. |
| `server/src/models/enclave.rs` | New: `Enclave`, `EnclaveRole`, `EnclaveMembership`, `EnclaveInvitation`. |
| `server/src/models/mod.rs` | Re-export new module. |
| `server/src/db/enclave.rs` | New: full CRUD + role + invitation helpers + `backfill_general_membership(auth, chat)`. |
| `server/src/main.rs` | Call `backfill_general_membership` after both pools migrate at startup. |
| `server/src/routes/auth.rs` | After auto-promoting the first registered user to site admin, call `backfill_general_membership` so the new admin owns General immediately. |
| `server/src/db/mod.rs` | Re-export. |
| `server/src/db/chat.rs` | `create_room` adds `enclave_id`; `list_rooms` becomes `list_rooms_in_enclave`; new `is_room_accessible`; unread-count helpers gain enclave filter. |
| `server/src/perms.rs` | New: enclave permission helpers. |
| `server/src/routes/enclave.rs` | New: routes listed above. |
| `server/src/routes/mod.rs` | Mount `enclave` module; replace `load_sidebar` with `load_chrome(state, user, current_enclave)`; remove the old single-sidebar helper. |
| `server/src/routes/home.rs` | Redirect via `last_visited`; render Home pseudo when no cookie. |
| `server/src/routes/room.rs` | Set `last_visited`; resolve enclave for sidebar; access check via `is_room_accessible`. |
| `server/src/routes/dm.rs` | Set `last_visited`. |
| `server/src/routes/search.rs` | Scope FTS to current enclave / Home DMs. |
| `server/src/routes/admin.rs` | Remove `POST /admin/rooms`; keep `GET /admin/rooms` grouped by enclave. |
| `server/src/views/enclave.rs` | New: view models for enclave templates. |
| `server/src/views/layout.rs` | Replace `SidebarRoom`/`SidebarPeer` glue with `ChromeView { switcher, sidebar }`. |
| `server/src/ws/events.rs` | New event variants. |
| `server/src/ws/*` | Broadcast targets for new events; OOB handling for switcher + sidebar. |
| `server/templates/layout.html` | Two-column chrome (switcher + sidebar). |
| `server/templates/partials/enclave_switcher.html` | New. |
| `server/templates/partials/enclave_sidebar.html` | New. |
| `server/templates/partials/sidebar.html` | Deleted. |
| `server/templates/enclave/*` | New templates listed above. |
| `server/templates/invitations/*` | New templates. |
| `server/templates/home/welcome.html` | Updated copy. |
| `server/templates/admin/rooms.html` | Grouped-by-enclave moderation view. |
| `server/tests/*` | New test files covering DB + integration + migration. |

## Out of Scope

(See "Non-Goals" above.)
