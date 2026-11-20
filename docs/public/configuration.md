# Configuration

Every environment variable is listed here, grouped by subsystem. Four variables are mandatory. Everything else has a default or gates an optional feature.

## Bunyip Single Sign-On (required)

All four of these must be set or the server refuses to boot.

| Variable | Purpose |
|---|---|
| `LETS_CHAT_BUNYIP_SSO_ISSUER` | The Bunyip OP's HTTPS issuer URL. Discovery and JWKS are fetched at boot. |
| `LETS_CHAT_BUNYIP_SSO_CLIENT_ID` | Confidential client id registered on the OP. |
| `LETS_CHAT_BUNYIP_SSO_CLIENT_SECRET` | Confidential client secret for `client_secret_basic` auth. |
| `LETS_CHAT_BUNYIP_SSO_REDIRECT_URI` | Must equal `<base>/auth/bunyip/callback`. Registered on the OP's `redirect_uris`. |

A dev-only switch `LETS_CHAT_BUNYIP_SSO_INSECURE_TLS=1` disables issuer TLS verification. Never set in production.

## HTTP server

| Variable | Default | Purpose |
|---|---|---|
| `BIND_ADDR` | `0.0.0.0:8080` | Socket the server listens on. |
| `LETS_CHAT_BASE_URL` | `http://localhost:8080` | Public base URL used in redirect URIs and email links. |

Trusting reverse-proxy headers (`X-Forwarded-For` / `X-Real-IP`) is an admin-panel toggle, `trust_proxy_headers`, stored in the database, not an environment variable.

## Data directories

| Variable | Default | Purpose |
|---|---|---|
| `LETS_CHAT_DATA_DIR` | `/data` | Root under which the SQLite files and upload trees live. User-uploaded files are stored under `${LETS_CHAT_DATA_DIR}/uploads`; this is not independently configurable. |

The data directory is created on demand if it does not already exist.

## Email ingress (optional)

Mail-in replies and email-to-room ingestion live on an IMAP poll loop. See [operations/email-ingress.md](operations/email-ingress.md) for the full operator setup. IMAP connection details (host, username, password, ingress domain) are admin-panel, database-backed settings, not environment variables.

## Outbound email (optional)

Needed for digests, notification emails, and reply-by-email.

| Variable | Purpose |
|---|---|
| `LETS_CHAT_SMTP_HOST` | SMTP relay hostname. |
| `LETS_CHAT_SMTP_USERNAME` | Relay credentials. |
| `LETS_CHAT_SMTP_PASSWORD` | Relay credentials. |
| `LETS_CHAT_SMTP_FROM` | `From:` header for outbound mail. |

## Voice and video (optional)

Lets chat uses a direct WebRTC mesh for small calls and a self-hosted LiveKit Selective Forwarding Unit for huddles and stage mode.

| Variable | Purpose |
|---|---|
| `LETS_CHAT_LIVEKIT_URL` | LiveKit server URL. |
| `LETS_CHAT_LIVEKIT_API_KEY` | LiveKit API key. |
| `LETS_CHAT_LIVEKIT_API_SECRET` | LiveKit API secret. |

## Transcription agent (optional)

Enables the server-side transcription path for call captions. The agent runs as its own container alongside the app.

| Variable | Purpose |
|---|---|
| `LETS_CHAT_TRANSCRIBE_AGENT_TOKEN` | Shared token the agent presents on callback. |
| `LETS_CHAT_TRANSCRIBE_AGENT_NAME` | Agent identifier. Defaults to `transcriber`. |

See [features/transcription-agent.md](features/transcription-agent.md).

## Semantic search (optional)

Lets chat's search is full-text by default; a configured embeddings endpoint adds semantic search and "Find related".

| Variable | Purpose |
|---|---|
| `LETS_CHAT_EMBEDDINGS_URL` | HTTP endpoint that returns a vector for text. |
| `LETS_CHAT_EMBEDDINGS_API_KEY` | Optional bearer token for the endpoint. |

## Protocol bridges (optional)

Register out-of-process bridge daemons. Each bridge gets a scoped API token and an outgoing webhook URL. See [operations/protocol-bridges.md](operations/protocol-bridges.md).

## Internationalization

The fallback locale is hardcoded to English; there is no environment variable to change it. The in-app picker and the browser's Accept-Language header take precedence over the fallback. English and Spanish ship today.

## Reference template

The `.env.standalone` file at the repo root is the annotated reference template: it names every variable with inline comments. Copy relevant values into your secrets store; do not deploy the file itself.
