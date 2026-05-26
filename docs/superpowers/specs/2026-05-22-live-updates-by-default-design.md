# Design: live updates by default (LC-156)

Status: proposed. Parent audit: `docs/audit/2026-05-22-lc148-audit-report.md` (gap C1).

## Problem

Live updates are opt-in per page. Only `room`, `dm`, and `voice` pages subscribe to the WebSocket, and each hand-rolls the same subscribe/teardown IIFE (3 identical copies). Every other page is static until a manual reload. Two failure modes:

1. A server event is broadcast but no page consumes its OOB fragment (`Enclave*`, `EnclaveInvitation*` reach only the sidebar; `/enclave/{id}` and `/invitations` stay stale).
2. A state change has no broadcast at all (admin lists, room info, search results, saved-message deletions, settings profile edits).

The root cause is that "be live" is a thing each page author must remember to wire, not a property the page inherits.

## Current mechanism (what we build on)

- Server: `ws::hub::Hub` with `broadcast_to_room`, `broadcast_to_room_except`, `broadcast_to_user`, `broadcast_global`. Events are the `ChatEvent` enum; each handler renders an HTML fragment tagged `hx-swap-oob` and the hub fans it to subscribed sockets.
- Client: htmx `ws` extension. A page sends a `{type:"subscribe", room_id}` frame on `htmx:wsOpen`; the server adds that socket to the room's subscriber set; OOB fragments merge into the DOM by element id.

The plumbing is sound. The gap is the per-page wiring and the missing topic granularity (only `room_id` subscription exists; there is no "enclave", "user-scoped page", or "admin" topic).

## Proposal

### 1. A declarative subscribe attribute

Replace the hand-rolled IIFE with a single delegated handler (one shared JS module, loaded in `base.html`) that reads declarative attributes off the page root:

```html
<div data-lc-live data-lc-topics="room:{{ room.id }}">...</div>
```

The shared module:
- On `htmx:wsOpen` (and immediately if `window.__lcWS` already open), sends a `subscribe` frame for each topic in `data-lc-topics`.
- On `htmx:beforeCleanupElement` of the `data-lc-live` root, sends an `unsubscribe` and tears down listeners (fixes the current per-page cleanup duplication).
- Idempotent re-subscribe on reconnect (reuses the existing reconnect path).

This deletes the 3 copy-paste blocks and makes "live" a one-attribute opt-in. New pages add `data-lc-live data-lc-topics="..."` and inherit correct subscribe/cleanup/reconnect behavior.

### 2. Generalize topics beyond room_id

Extend the server subscribe frame + hub to support typed topics:
- `room:{id}` (existing behavior).
- `enclave:{id}` -> drives `/enclave/{id}` member/room lists and `/invitations`.
- `user:{id}` -> per-user page surfaces (settings, inbox, saved, activity) that should reflect the viewer's own changes across tabs. (`broadcast_to_user` already targets the socket; the page just needs to consume the OOB fragment.)
- `admin` -> admin list pages subscribe; admin-relevant events (ban/mute/role/room-count) broadcast to it.

Hub stores `DashMap<Topic, Vec<Sender>>` instead of room-only. Membership/authorization is checked at subscribe time per topic (mirrors the existing `is_room_member` check for private rooms) so a user cannot subscribe to an `admin` or foreign `enclave`/`user` topic.

### 3. Fill the missing broadcasts + OOB consumers

Per the audit gap list, for each stale surface either (a) add an OOB fragment consumer for an event that already broadcasts, or (b) add the missing broadcast. Concretely:
- `/enclave/{id}`: consume `EnclaveMember*` / `EnclaveRoom*` (already broadcast) by rendering OOB list rows; subscribe via `enclave:{id}`.
- `/invitations`: consume `EnclaveInvitation*`.
- `/admin/users`, `/admin/rooms`: add `admin` topic + OOB rows for ban/mute/role/count changes.
- `/settings`: OOB the viewer's own profile/status edits via `user:{id}`.
- `/saved`, `/inbox`, `/activity`: consume the relevant per-user events via `user:{id}`.

### 4. Authorization at subscribe time

Every topic subscribe is access-checked server-side (room membership, enclave membership, self-only for `user:`, admin role for `admin`). This is the same gate the audit flagged as missing in places (see S1) - centralizing subscription auth here avoids re-deriving it per surface.

## Migration

1. Land the shared `data-lc-live` module + topic generalization (no behavior change: `room`/`dm`/`voice` switch to the attribute, delete their IIFEs).
2. Add topics + OOB consumers surface-by-surface (each its own small PR), highest-traffic stale page first (`/enclave/{id}`, then `/invitations`, then admin).

## Acceptance

- New page becomes live by adding one attribute; no JS copy-paste.
- The audit's stale-surface list is driven to zero (or each remaining static surface has a documented reason).
- Subscribe authorization is enforced for every topic kind.

## Out of scope

- Client-side rendering / a SPA rewrite. This stays server-rendered-HTML-over-WS (the existing model); the change is making subscription declarative and topic-typed.

## Shipped

Delivered across LC-156 (declarative subscribe via `live.js` + `data-lc-live-room`/`data-lc-live-topic`), LC-160 (typed topics `enclave:{id}`/`user:{id}`/`admin` + subscribe-time authz), and the per-surface fills: LC-161 invitations, LC-170 enclave landing member/room lists, LC-172 settings member list, LC-173 own-profile sidebar block, LC-174 sidebar room nav, LC-175 admin user list, LC-177 admin room list, LC-178 `/saved`. LC-176 added `Hub::unsubscribe_user_from_topic` for access-loss cleanup.

Two deviations from the original sketch, both forced by reality and documented in `docs/ui-conventions.md`:

- The "one wrapper subscribes the page and merges fragments" idea landed as **id-keyed OOB regions** rather than a generic merge wrapper: the live fragment swaps an element by id and htmx drops it when the id is absent, which self-limits delivery to the right page without per-connection page tracking and survives stale subscriptions.
- **Paginated / filtered surfaces** (`/inbox` infinite-scroll, `/activity` tabs, LC-179) carry view-state the server can't see, so they use a reveal-a-refresh-bar affordance instead of an auto full-list swap. Author-row cross-surface refresh on profile edits (other users' views of your avatar/name) was explicitly deferred as the expensive case (LC-173 phase 3).

The audit's stale-surface list is at zero for the surfaces with a per-user or topic broadcast; the remaining gap (replies/reactions to your own messages revealing the `/activity` bar) is noted in LC-179 as needing a new per-user event.
