# Lets chat

Self-hosted team chat for Managed Service Providers. Rooms, direct messages, voice and video calls, screen share, a desktop app, and a JSON HTTP API, all shipped as a single Rust binary.

## Who this is for

- **MSP operators** standing up a chat server for their technicians and clients. Single-binary deploy, docker compose stack, mandatory Bunyip Single Sign-On.
- **MSP administrators** managing enclaves (grouped workspaces), rooms, user groups and roles for their organization.
- **MSP technicians and clients** using the web or desktop app day-to-day: chatting, calling, sharing files, searching history.

## When NOT to use it

- As a public, open-registration chat service. Sign-in is through Bunyip only; there is no local registration path, no username / password login, no password reset. An account exists because someone with a Bunyip identity signed in.
- As a federated chat replacement. The Matrix bridge is an operator-run daemon translating between a single room and a Matrix channel, not full federation.
- For audiences that need end-to-end encryption. Lets chat stores messages and attachments at rest; the operator holds the keys.

## What ships today

Chat features, calls, email ingress, protocol bridges (Matrix), a server-side transcription agent (optional), the desktop app (Windows, macOS, Linux), the web app, the JSON HTTP API, English and Spanish localization, and administrative backup and restore.

## Where to start

- **Operator, first deployment**: [Getting started](getting-started.md), then [Installation](installation.md) and [Configuration](configuration.md).
- **Operator, OIDC wiring**: [OIDC integration](oidc-integration.md).
- **End user**: [Features overview](features/overview.md), then the feature pages under it.
- **Operator, something is wrong**: [Troubleshooting](troubleshooting.md).

Everything on docs.a8n.systems under `/docs/lets-chat` is current shipping behavior. Internal design material and roadmap live outside this tree and are not published here.
