# lets-chat HTTP API (v1)

A small, stable JSON API for bots, scripts, CI, and other machine clients.
Authentication is by **personal API token** (LC-72), not the browser session
cookie. Tokens are minted at **Settings -> API tokens** and shown exactly once.

## Authentication

Send the token as a bearer header:

```
Authorization: Bearer lc_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
```

- A missing, unknown, expired, or revoked token returns **401 Unauthorized**.
- A valid token that lacks the route's required scope returns **403 Forbidden**.
- Revocation takes effect immediately. Expiry is enforced server-side.
- The API is only available when the server has `LETS_CHAT_SECRET_KEY` set
  (the key that HMACs stored token hashes). Without it, all token auth is 401.

A token never grants more than its owning user already has: scopes *narrow*
access (e.g. read-only), and every route still enforces the user's own room
membership / access rules on top of the scope check.

The scheme is case-insensitive (`Bearer`, `bearer`, `BEARER` all work).

### Bots (LC-73)

A bot is a first-class non-human account. An admin creates one at **Admin -> Bots**; creation mints an initial API token (shown once) that authenticates exactly like a user token via the same `Authorization: Bearer` header and scope model. Messages a bot posts are attributed to the bot identity (name + avatar), and the same room-access and ban/mute rules apply.

> **Maintenance mode:** the API is **not** gated by maintenance mode - bearer
> requests keep working while the web UI shows the maintenance page. This is
> intentional so bots and integrations are not knocked offline by a UI
> maintenance window; revoke tokens (or stop the server) to halt API traffic.

## Scopes

| Scope | Grants | Bridge bots |
|-------|--------|-------------|
| `messages:read`    | Read messages in rooms the user can access. | Refused - `require_not_bridge` rejects any bridge-role token with 403, even if granted. |
| `messages:write`   | Post messages in rooms the user can access. | Refused - same `require_not_bridge` gate. |
| `rooms:read`       | List rooms the user can see. | Refused - same `require_not_bridge` gate. |
| `bridge:post`      | Post a foreign-protocol message under a caller-chosen display name on a bridge the bot owns. | Bridge-only; see `docs/protocol-bridges.md`. |
| `bridge:heartbeat` | Record a liveness ping for a bridge the bot owns. | Bridge-only; see `docs/protocol-bridges.md`. |

## Endpoints

| Method | Path | Required scope | Description |
|--------|------|----------------|-------------|
| GET  | `/api/v1/me` | (any valid token) | The token owner's identity (`id`, `username`, `role`). |
| GET  | `/api/v1/rooms` | `rooms:read` | Non-DM rooms the user can see (`id`, `name`, `room_type`). |
| GET  | `/api/v1/rooms/{room_id}/messages` | `messages:read` | Paginated top-level messages in a room, newest first. Returns an envelope `{"messages": [...], "next_cursor": ...}` where each message has `id`, `room_id`, `user_id`, `author`, `body`, `created_at`. Query parameters: `before_id` (optional cursor - return messages strictly older than this id; omit to start at the most recent) and `limit` (optional page size, default 50, clamped to `[1, 200]`). `next_cursor` is the smallest `id` returned, to feed back as `before_id` on the next request; it is `null` when the page returned fewer than `limit` rows (history exhausted) or the room is empty. |
| POST | `/api/v1/rooms/{room_id}/messages` | `messages:write` | Post a message. JSON body `{"body": "..."}`. Returns the created message. Enforces the same send gates as the web composer (ban/mute, room access, rate limit, per-enclave burst override, enclave ban, posting policy, slowmode, new-member cooldown, DM block, link filter); broadcasts to connected clients. A policy/ban/block denial is **403 Forbidden**, a rate-limit/slowmode/cooldown denial is **429 Too Many Requests** with a `Retry-After` header, and a blocked-link body is **400 Bad Request**. |

`messages:read`, `messages:write`, and `rooms:read` are refused for bridge-role tokens (see the scope table above). Bridge bots instead reach `/api/v1/bridges/{id}/messages` and `/api/v1/bridges/{id}/heartbeat`, documented in `docs/protocol-bridges.md` along with the outgoing-webhook stream a bridge daemon subscribes to for reads. Routes that appear in neither this table nor `docs/protocol-bridges.md`'s API surface table are not reachable with an API token.

### Examples

```sh
# Identity
curl -H "Authorization: Bearer $TOKEN" https://chat.example/api/v1/me

# List rooms
curl -H "Authorization: Bearer $TOKEN" https://chat.example/api/v1/rooms

# Post a message
curl -X POST https://chat.example/api/v1/rooms/1/messages \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"body":"hello from a bot"}'

# Page through a room's history, newest first
curl -H "Authorization: Bearer $TOKEN" \
  "https://chat.example/api/v1/rooms/1/messages?limit=50"
# => {"messages": [...50 rows...], "next_cursor": 214}

curl -H "Authorization: Bearer $TOKEN" \
  "https://chat.example/api/v1/rooms/1/messages?limit=50&before_id=214"
# => {"messages": [...older rows...], "next_cursor": null}   # history exhausted
```

## Incoming webhooks (LC-74)

A separate, **unauthenticated** ingress for external systems (Grafana, CI,
alerting) that speak HTTP + JSON but cannot hold a bearer token. A room
moderator creates a webhook at **#room -> Moderators -> Manage incoming
webhooks**; the secret URL is the credential and is shown exactly once.

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | `/webhook/{secret}` | the secret in the URL | Append a message to the webhook's room, attributed to the webhook (name + optional avatar), not a user. |

Request body:

```json
{ "text": "alert fired", "markdown": true }
```

- `text` (required): the message body. `markdown` (optional, default `false`):
  when `true` the text renders through the markdown pipeline; otherwise it is
  escaped and renders literally.
- **401** unknown/invalid secret, **410** revoked webhook, **429** (with
  `Retry-After`) past the per-webhook rate cap (60/min). **204** on success.
- Requires `LETS_CHAT_SECRET_KEY` (only an HMAC of the secret is stored). The
  secret never appears in request logs.

```sh
curl -X POST https://chat.example/webhook/lc_xxxxxxxx \
  -H "Content-Type: application/json" \
  -d '{"text":"deploy finished :rocket:","markdown":true}'
```

## Outgoing webhooks (LC-75)

Event subscriptions: an admin registers a delivery URL + event filter + scope
at **Admin -> Webhooks**. When a matching event fires, the server POSTs a
signed JSON body to the URL. Events: `message.posted`, `message.edited`,
`message.deleted`, `reaction.added`. Scopes: `global`, `enclave` (id), `room` (id).

Payload (stable, versioned):

```json
{ "version": "1", "event": "message.posted", "room_id": 1, "data": { "...": "..." } }
```

Each POST carries:

- `X-LetsChat-Event` - the event name.
- `X-LetsChat-Timestamp` - unix seconds (use for replay protection).
- `X-LetsChat-Signature: sha256=<hmac>` - HMAC-SHA256 over the **raw body**,
  keyed by the webhook's signing secret (shown once at creation, rotatable).

Verify by recomputing the HMAC and comparing. Delivery is at-least-once with
retries (1s, 4s, 16s, 1m, 5m, 30m; 6 attempts). After repeated failed
deliveries the webhook auto-disables; an admin can re-enable it. Per-webhook
delivery history is visible in the admin UI. URLs and secrets are never logged.
