# Integrations

External systems lets chat talks to. Each entry: what it is for, what to configure, what breaks if it is unreachable.

## Bunyip (required)

The authentication path. Lets chat is a confidential OIDC client; Bunyip is the OpenID Connect provider. See [OIDC integration](oidc-integration.md).

What to configure: the four `LETS_CHAT_BUNYIP_SSO_*` variables and the client registration on the Bunyip side.

What breaks if Bunyip is unreachable at boot: the server refuses to start. Discovery and JWKS fetch at boot are fatal if they fail.

What breaks if Bunyip is unreachable at runtime: existing sessions keep working until their cookie expires or an admin revokes them. New sign-ins fail at the OP step; the user sees a Bunyip error page. Token refreshes to the OP fail silently and the session falls back to its cached state until it expires.

## LiveKit (optional)

The Selective Forwarding Unit for calls above a handful of participants and for stage mode. See `LETS_CHAT_LIVEKIT_*` in [Configuration](configuration.md).

What breaks if LiveKit is unreachable: 1:1 calls over the mesh path still work. Huddles above the mesh cap and stage mode degrade: new participants get an error on join, existing participants keep streaming until they disconnect.

## Embeddings endpoint (optional)

A URL that takes text and returns a vector. Any OpenAI-compatible embeddings endpoint works. Configure `LETS_CHAT_EMBEDDINGS_URL` and optionally `LETS_CHAT_EMBEDDINGS_API_KEY`.

What breaks if the endpoint is unreachable: semantic search falls back silently to lexical search. New messages' embeddings are retried in the background; the index catches up when the endpoint comes back.

## IMAP mail server (optional)

The source of mail ingress and reply-by-email. See [operations/email-ingress.md](operations/email-ingress.md).

What breaks if the IMAP server is unreachable: the poll loop logs a warning and retries at the next tick. Inbound messages queue on the mail provider side until lets chat catches up.

## SMTP relay (optional)

Outbound mail: notification emails, email digests, reply-by-email bounces.

What breaks if the SMTP relay is unreachable: outbound mail queues to a bounded local queue and retries. If the queue fills, new outbound mail is dropped and a warning is logged. The queue is per-process and does not survive a restart.

## Protocol bridges (optional)

Operator-run daemons translating between lets chat and foreign protocols. See [operations/protocol-bridges.md](operations/protocol-bridges.md).

What breaks if a daemon dies: the server stops delivering webhook events to it after three missed heartbeats. Inbound events from the foreign protocol stop reaching lets chat until the daemon is back.

## Transcription agent (optional)

A separately-deployed container that joins calls to produce captions and post-call transcripts. See [features/transcription-agent.md](features/transcription-agent.md).

What breaks if the agent is down: calls work; captions do not. The browser-side Web Speech captions still work for users whose browser supports them.
