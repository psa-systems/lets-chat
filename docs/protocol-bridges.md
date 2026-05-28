# Protocol bridges (LC-78)

LC-78 lets an out-of-process daemon translate between lets-chat and a foreign protocol (Matrix in v1; IRC and XMPP defer freely on the same schema). The lets-chat server provides the registration surface, a scoped API for the daemon to post / heartbeat, and an outgoing-webhook stream the daemon subscribes to for outbound events. The daemon itself is documented but NOT shipped: an operator runs `matrix-appservice-bridge` or any community daemon configured against this API.

## Loop-break: read this first

> **A daemon that does not self-filter its own outgoing-webhook traffic creates an infinite cross-network amplification loop.**
>
> Every `message.posted` / `message.edited` / `message.deleted` / `reaction.added` payload carries an `actor` block describing the original author. A daemon MUST drop any event where:
>
> ```
> actor.kind == "bridge" AND actor.bridge_id == <THIS DAEMON'S registered bridges.id>
> ```
>
> Without this filter, every message the daemon posts to lets-chat fires an outgoing webhook, the daemon receives it, the daemon re-pushes to the foreign network, the foreign network sends it back, the daemon posts it again, and the loop runs as fast as the network allows until either side rate-limits or runs out of resources. **Always test the filter on a low-volume room first.**
>
> The `bridge_id` you filter on is the PERSISTENT `bridges.id` integer from this server, NOT a session token. It is stable across daemon restart and token rotation. Read it once at startup from your `bridges` row (via the admin UI on bridge creation, or persisted in the daemon's local config) and use it for the lifetime of the daemon.

## What this is, and what it isn't

**Is**: a registration + transport surface. The server holds the bridge row (room, kind, sealed daemon config, bot identity, heartbeat status), exposes `POST /api/v1/bridges/{id}/messages` for the daemon to inject foreign messages, exposes `POST /api/v1/bridges/{id}/heartbeat` for liveness, and fires LC-75 outgoing webhooks the daemon subscribes to for outbound events.

**Isn't**: a Matrix appservice. lets-chat does not implement the Matrix Client-Server or Appservice protocols, does not federate, does not handle E2EE, and does not own state for the Matrix room. All Matrix-side state lives in the homeserver; lets-chat-side state lives in the chat room. The daemon is the translator and persists nothing of its own beyond what's needed to identify itself.

**Isn't**: a sidekick lets-chat ships. v1 does not include a Matrix daemon binary or compose service. The operator runs a community daemon (`matrix-appservice-bridge`, `mx-puppet-discord`, or anything that can call this API) on their own infrastructure.

## v2 release-notes header: foreign-avatar proxy is ON by default

> **Behavior change on upgrade.** LC-78-AVATAR-PROXY ships with foreign-avatar fetching **enabled by default**. The v1 reject-non-null behavior is preserved behind the `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false` env var; set this BEFORE upgrading if you want v1's posture back. Daemons that observed v1's 400 may now start submitting `foreign_avatar` URLs (some daemon configs disabled the field client-side based on the v1 reject; if you bumped your daemon's config to omit the field, upgrading the server alone is sufficient).
>
> The proxy fetches each foreign avatar exactly once, caches the bytes server-side, and serves them from a same-origin URL (`/media/bridge-avatar-proxy/{hash}`). Viewers' browsers never hit the foreign homeserver. The structural side-channel closure: the foreign URL never appears in rendered HTML, only the opaque sha256 hash does. Reading the HTML reveals nothing about which homeservers the room bridges with.

## v1 + v2 scope (what works, what does not)

| Capability | Status | Notes |
|---|---|---|
| Matrix → lets-chat new messages | yes (v1) | Daemon POSTs `/api/v1/bridges/{id}/messages` with `body` + `foreign_name`. |
| lets-chat → Matrix new messages | yes (v1) | Daemon subscribes to LC-75 `message.posted` and translates. |
| Matrix per-user actor identity in lets-chat | yes (v1) | `foreign_name` snapshots on the row; "alice (via matrix)" renders. |
| Foreign avatars | **proxied (v2)** | Server fetches once via `bridge_avatar::fetch_and_cache`, magic-byte-sniffs, re-encodes through the uploads pipeline (EXIF / XMP / IPTC strip), stores on disk, serves from `/media/bridge-avatar-proxy/{hash}`. Pending fetch and failed fetch both 404 from the proxy endpoint; the `<img onerror>` falls back to initials. v1's reject behavior is restored by `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false`. |
| Edits / deletes (either direction) | **deferred** | LC-75 fires `message.edited` and `message.deleted` with the `actor` block, so the daemon CAN see them; v1 daemons are expected to ignore. Pushing Matrix `m.replace` events to lets-chat needs an `/api/v1/bridges/{id}/messages/{mid}/edit` endpoint that v1 does not ship. |
| Reactions | **deferred** | LC-75 fires `reaction.added`; v1 daemon ignores. Matrix reaction model differs from lets-chat's. |
| Threads | **deferred** | Matrix threading model differs. |
| File / attachment relay | **deferred** | Each direction is its own sub-protocol. |
| Identity mapping (`@alice:matrix.org` → lets-chat user) | **rejected** | Account linking is a security surface (impersonation). v1 keeps foreign users as display-only synthetic actors. |
| Encrypted Matrix rooms | **rejected** | Key management is its own sub-feature. |
| IRC / XMPP daemons | **out of scope** | Schema is `kind`-agnostic; their daemons land on the same registration surface as Matrix when written. |

## Threat model

- **The bridge daemon is the LEAST-trusted component.** It runs out-of-process, operator-managed, possibly on different infrastructure than the chat server. A compromised daemon's blast radius is its bot account; the bot is gated to the `bridge` role tier (defense in depth on top of token scoping), which restricts it to `bridge:post` and `bridge:heartbeat` on its OWNED bridges only.

- **The bot's role tier denies everything else.** Even if an operator mistakenly grants a bridge-role token `messages:write` or `messages:read`, the server's per-endpoint `require_not_bridge()` gate rejects the call with 403. Verified by `routes_bridge_role_isolation`.

- **`config_encrypted` is a real credential.** The Matrix homeserver shared secret (or equivalent for other protocols) is stored AES-256-GCM-sealed under `LETS_CHAT_SECRET_KEY` with a separate nonce column, same convention as `imap_inbox_config` (LC-77) and `vapid_keys`. A chat.db leak cannot reconstruct usable Matrix tokens.

- **Foreign avatars are rejected outright (v1).** A bridge message's `foreign_avatar` is foreign-controlled and would point at an arbitrary federated homeserver. Every render would make the viewer's browser fetch from that homeserver, leaking the viewer's IP, User-Agent, and `Referer` to anyone who registered a federated homeserver. v1 fails loud (HTTP 400 with `LC-78-AVATAR-PROXY` token) rather than silently dropping, so a misconfigured daemon surfaces the policy. The follow-up to proxy-cache foreign avatars is tracked separately.

- **Cookie login refuses bots.** A bridge bot has `is_bot = 1`; the cookie-login flow rejects it (LC-73 invariant). The bot can ONLY authenticate via the scoped API token. There is no path that turns a leaked bot token into a session.

- **The per-message actor override is privilege-isolated.** `bridge:post` is a strictly more powerful scope than `messages:write`: the caller chooses the rendered display name. The endpoint sits on its own route (`/api/v1/bridges/{id}/messages`) gated to its own scope and rejects any token that holds `messages:write` but not `bridge:post`. The separation IS the security boundary.

- **Stop-new removal preserves history.** Removing a bridge from `/admin/bridges` DELETEs the bridges row, which triggers `ON DELETE SET NULL` on `messages.bridge_id` (the FK). The snapshotted `bridge_foreign_name` + `bridge_kind` columns persist so historical messages still render as "alice (via matrix)". This was a criterion-owner-deferred decision made on the principle that the mistake recoverable in code (stop-new flipped later to delete-history is an additive branch) is preferred over the mistake unrecoverable in data.

## API surface

| Endpoint | Method | Scope | Auth | Body | Returns |
|---|---|---|---|---|---|
| `/api/v1/bridges/{id}/messages` | POST | `bridge:post` | Bearer (bot must own bridge) | `{body, foreign_name, foreign_avatar?}` | `ApiMessage` JSON |
| `/api/v1/bridges/{id}/heartbeat` | POST | `bridge:heartbeat` | Bearer (bot must own bridge) | `{error?}` (optional, empty OK) | `{ok, status}` |
| `/api/v1/me` | GET | (any token) | Bearer | - | `ApiMe` |

The bridge bot's token grants exactly the two `bridge:*` scopes. `messages:write` is NEVER mixed in: the bridge endpoint sits on its own scope to keep per-message actor override out of reach of generic `messages:write` callers.

### Posting a bridge message

```
POST /api/v1/bridges/42/messages
Authorization: Bearer lc_...
Content-Type: application/json

{"body": "hello from matrix", "foreign_name": "alice:matrix.org"}
```

Validation:

- `body`: trimmed, non-empty, bounded by the LC-153 message length cap.
- `foreign_name`: trimmed, non-empty, max 256 bytes.
- `foreign_avatar`: must be absent or null in v1. Any non-null value is rejected with HTTP 400 and an `LC-78-AVATAR-PROXY` token in the error body.

Effects:

- Inserts a `messages` row with `bridge_id`, `bridge_foreign_name`, `bridge_kind` snapshotted from the bridge row + POST body.
- Broadcasts to connected lets-chat clients as a normal HTMX OOB fragment with the synthetic actor.
- Fires LC-75 `message.posted` with the `actor` block (see below).

### Heartbeating

```
POST /api/v1/bridges/42/heartbeat
Authorization: Bearer lc_...
Content-Type: application/json

{}
```

Optional `{"error": "homeserver unreachable"}` body to surface a daemon-side fault. With error, the bridge's status becomes `errored` and `last_error` is stored (truncated at 4096 bytes; the operator can read it from `/admin/bridges`). Without error, status becomes `healthy` and `last_error` is cleared. The admin UI computes `stale` when `last_heartbeat_at` is older than 90 seconds (3× a typical 30s daemon interval).

### Outgoing-webhook payload

Subscribe an outgoing webhook (LC-75) at `/admin/outgoing-webhooks` to whichever events the daemon needs. The payload now carries an `actor` block on every message-* and reaction.added event:

```json
{
  "version": 1,
  "event": "message.posted",
  "room_id": 5,
  "data": {
    "message_id": 123,
    "body": "hello",
    "actor": {
      "kind": "bridge",
      "bridge_id": 42
    }
  }
}
```

Shapes:

- User-authored: `"actor": {"kind": "user", "user_id": "..."}`.
- Webhook-authored (LC-74): `"actor": {"kind": "webhook", "webhook_id": N}`.
- Email-ingress-authored (LC-77): `"actor": {"kind": "email_inbox", "email_inbox_id": N}` (when the email path's LC-75 wiring lands; see "Known gaps" below).
- Bridge-authored (LC-78): `"actor": {"kind": "bridge", "bridge_id": N}`. Same shape on `message.edited` and `message.deleted` events for messages the bridge originally posted (the actor describes the ORIGINAL author, not the editor/deleter).

## Operator workflow

1. **Register the bridge.** Go to `/admin/bridges` → `Register a bridge`. Pick the target room, give the bot a username (e.g. `matrix-bridge`), select `matrix`, paste the daemon's config (a JSON blob the daemon will read back). On submit the page shows the one-time API token. **Copy it now.** It is not shown again.

2. **Configure the daemon.** Point the daemon at this server's `/api/v1/bridges/{id}/messages` for posting and `/api/v1/bridges/{id}/heartbeat` for liveness. The bridge id is the integer the URL path shows on `/admin/bridges` (or the row id in the `bridges` table). The token is the one shown above.

3. **Subscribe to outbound events.** Create an outgoing-webhook subscription at `/admin/outgoing-webhooks` scoped to the bridged room (or globally), pick whichever events the daemon needs. Point it at the daemon's HTTPS endpoint. The daemon must validate the `X-LetsChat-Signature` HMAC against the subscription's signing secret.

4. **Implement the loop-break filter.** Before pushing any event to the foreign network, drop events where `actor.kind == "bridge" && actor.bridge_id == <your bridge id>`. See the warning at the top of this document.

5. **Test on a low-volume room.** A loop on a low-volume room is recoverable. A loop on a busy room is an incident.

6. **Operate.** The admin UI shows `pending` until the first heartbeat, `healthy` while heartbeats are recent and error-free, `errored` after a heartbeat that reported an error, `stale` when no heartbeat has arrived in 90 seconds.

7. **Reconfigure.** No in-place edit in v1: remove the bridge and create a new one. The bot is left orphaned (its bridge-scoped token can't reach any bridge); disable it from `/admin/bots` if you want explicit cleanup.

## Removal semantics (stop-new)

Removing a bridge from `/admin/bridges`:

- DELETEs the `bridges` row.
- `ON DELETE SET NULL` clears `messages.bridge_id` on every message authored by that bridge.
- `bridge_foreign_name` and `bridge_kind` snapshots PERSIST so historical messages still render with the foreign actor identity.
- Does NOT ban the bot or revoke its API token. The token's scopes are bridge-only and now have no bridge to act on; disable the bot via `/admin/bots` if you want explicit cleanup.

This is the LC-78 v1 design: stop-new, not delete-history. The principle: the wrong choice made in code (stop-new flipped to delete-history later) is reversible via an additive admin branch; the wrong choice made in data (history hard-deleted on every removal) is permanent. If your deployment ever needs strict delete-history semantics, it's an additive follow-up, not a destructive change to v1.

## Known gaps

- **Email-ingress (LC-77) does not fire LC-75 outgoing webhooks.** The `finalize_email_inbox_message_send` helper broadcasts to WS and fans mentions but does not enqueue a `message.posted` event. This is a pre-existing LC-75 coverage gap that pre-dates LC-78; a bridge daemon will not see email-ingress messages on the outgoing-webhook stream. Tracked separately.

- **`message.edited` / `message.deleted` propagation is daemon-ignore in v1.** lets-chat fires the event with the bridge actor block, but a v1 daemon has no API to push edits/deletes back. Ignore the events or surface them as informational; don't act on them.

- **No bridge-config edit endpoint.** Reconfigure = remove + recreate. Config rotation (e.g., rotating the Matrix shared secret) requires a new bridge id, which means daemon downtime. Acceptable for v1; a future `PATCH /admin/bridges/{id}/config` endpoint would close this without changing the snapshot model.
