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
