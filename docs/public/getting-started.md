# Getting started

Smallest working setup: a container pointed at a Bunyip OIDC provider, with a technician signing in through the web UI.

## Prerequisites

- A host that can run Docker, with outbound HTTPS to the Bunyip issuer.
- A Bunyip OP (a running Bunyip instance acting as the OIDC provider) with a registered confidential client. See [OIDC integration](oidc-integration.md).
- A public hostname that resolves to the host and a TLS certificate. For a first test the cert can be self-signed or a Let's Encrypt staging cert; for production get a real one.

## Four mandatory values

Lets chat refuses to start without all four. Everything else has a default or gates an optional feature.

```
LETS_CHAT_BUNYIP_SSO_ISSUER=https://your-op.example.com
LETS_CHAT_BUNYIP_SSO_CLIENT_ID=<the id Bunyip gave you>
LETS_CHAT_BUNYIP_SSO_CLIENT_SECRET=<the secret Bunyip gave you>
LETS_CHAT_BUNYIP_SSO_REDIRECT_URI=https://chat.example.com/auth/bunyip/callback
```

## Bring the server up

With the four values set:

```
docker run -d \
  --name lets-chat \
  -p 443:443 \
  -v /srv/lets-chat/data:/data \
  --env-file ./lets-chat.env \
  dev.a8n.run/psa-systems-private/lets-chat:latest
```

The container writes SQLite databases and uploaded files under `/data`. Mount that path on durable storage (not a tmpfs).

The server's health probe is at `GET /healthz`. The orchestrator should treat non-200 as unhealthy; a clean boot takes one or two seconds after the container starts.

## First sign-in

Open the hostname in a browser. The sign-in page sends you to the Bunyip OP; after Bunyip approves you, Lets chat lands you on the home room list.

The first account to sign in is promoted to administrator. Use that account to create the first enclave, invite other members, and configure optional features at `/admin`.

## Install the desktop app

The desktop app wraps the same web UI in a native shell. Operators distribute a signed binary per platform. See [Desktop app](desktop-app.md) for the installer paths and the update flow.

## Next steps

- Reference every available environment variable in [Configuration](configuration.md).
- Review the [Features overview](features/overview.md) so you know what to enable and when.
- Plan backups: [operations/backups.md](operations/backups.md).
