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

| Scope | Grants |
|-------|--------|
| `messages:read`  | Read messages in rooms the user can access. |
| `messages:write` | Post messages in rooms the user can access. |
| `rooms:read`     | List rooms the user can see. |

## Endpoints

| Method | Path | Required scope | Description |
|--------|------|----------------|-------------|
| GET  | `/api/v1/me` | (any valid token) | The token owner's identity (`id`, `username`, `role`). |
| GET  | `/api/v1/rooms` | `rooms:read` | Non-DM rooms the user can see (`id`, `name`, `room_type`). |
| GET  | `/api/v1/rooms/{room_id}/messages` | `messages:read` | Top-level messages in a room (`id`, `room_id`, `user_id`, `author`, `body`, `created_at`). |
| POST | `/api/v1/rooms/{room_id}/messages` | `messages:write` | Post a message. JSON body `{"body": "..."}`. Returns the created message. Honors ban/mute + room access; broadcasts to connected clients. |

Routes that do not appear here are not reachable with an API token.

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
