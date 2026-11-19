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
| `LETS_CHAT_BIND` | `0.0.0.0:443` | Socket the server listens on. |
| `LETS_CHAT_PUBLIC_URL` | derived | Public base URL used in redirect URIs and email links. |
| `LETS_CHAT_TRUSTED_PROXIES` | empty | CIDRs the server treats as trusted reverse proxies for `X-Forwarded-*`. |

## Data directories

| Variable | Default | Purpose |
|---|---|---|
| `LETS_CHAT_DATA_DIR` | `/data` | Root under which the SQLite files and upload trees live. |
| `LETS_CHAT_UPLOADS_DIR` | `${DATA_DIR}/uploads` | Where user-uploaded files are stored. |

The data directory must exist and be writable at boot. A missing directory is a fatal error, not an auto-create.

## Email ingress (optional)

Mail-in replies and email-to-room ingestion live on an IMAP poll loop. See [operations/email-ingress.md](operations/email-ingress.md) for the full operator setup.

| Variable | Purpose |
|---|---|
| `LETS_CHAT_EMAIL_INGRESS_IMAP_HOST` | IMAP server hostname. |
| `LETS_CHAT_EMAIL_INGRESS_IMAP_USER` | Mailbox username. |
| `LETS_CHAT_EMAIL_INGRESS_IMAP_PASSWORD` | Mailbox password. |
| `LETS_CHAT_EMAIL_INGRESS_DOMAIN` | Hostname used in the `<token>@<domain>` recipient address. |

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

`LETS_CHAT_DEFAULT_LOCALE` sets the fallback locale. The in-app picker and the browser's Accept-Language header take precedence. English and Spanish ship today.

## Reference template

The `.env.standalone` file at the repo root is the annotated reference template: it names every variable with inline comments. Copy relevant values into your secrets store; do not deploy the file itself.
