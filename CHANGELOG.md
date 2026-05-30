# Changelog

Operator-facing record of changes that affect how you **run, configure, secure, or upgrade** lets-chat. If you operate a deployment, read the Security and Changed entries before upgrading: they call out default-on behavior changes and "set this env var / upgrade promptly" actions.

This file records **tagged releases**. The project's release flow (`just create-release`, see `docs/releasing.md`) bumps the version, tags, and publishes; this file is curated at that point from the operator-action markers in git history. Between releases, the operator-action delta is always reconstructable from git and never lives only here:

```
git log --grep='\[operator-action\]' <last-tag>..HEAD
```

Format loosely follows [Keep a Changelog](https://keepachangelog.com). Sections: **Security** (must-act), **Changed** (behavior/default/config changes), **Added**, **Fixed**, **Deprecated**. Internal-only work (refactors, test hygiene, decoder hardening with no operator impact) is intentionally omitted; the git history is the complete record.

## [Unreleased]

No tagged release has been cut yet: the project currently ships the `latest` OCI image built off `main`, and the desktop self-updater only publishes on `v*` tags. The operator-visible changes that predate the first tag are captured under **Pre-release** below. New operator-action changes land in git with the `[operator-action]` marker (see `docs/releasing.md`) and are folded into the next version section when a release is cut.

## Pre-release - operator-visible changes to date (snapshot 2026-05-30)

Seed backfill for operators running `latest` off `main`. Not a complete history; only changes that affect how you run or secure a deployment. Lead items first.

### Security

- **Unified outbound SSRF guard; closed an unguarded Web Push SSRF (LC-152).** All server-initiated outbound HTTP (outgoing webhooks, Web Push, bridge-avatar fetch) now routes through a single guarded client that refuses connections resolving to private / non-public addresses. The audit that motivated this found Web Push (`push/mod.rs`) had **no SSRF guard at all** in shipped versions, so a crafted push endpoint could reach internal-network addresses and exfiltrate response metadata. **Upgrade promptly** if your deployment has Web Push or outgoing webhooks enabled.

### Changed

- **Foreign bridge-avatar proxy fetching is default-ON (LC-78-AVATAR-PROXY).** When a protocol bridge submits a foreign avatar URL, the server now fetches it server-side (capped, SSRF-guarded, re-encoded) and serves it same-origin, instead of rejecting it. This means **outbound fetches to foreign homeservers happen by default** on bridge traffic. To restore the v1 reject-non-null behavior, set `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false` (or `0`).
- **Destructive message-retention sweep, gated default-OFF (`LETS_CHAT_RETENTION_SWEEP_ENABLED`).** A background sweep that **hard-deletes** messages past a room's `retention_days` is shipped but disabled unless you set `LETS_CHAT_RETENTION_SWEEP_ENABLED=1` (or `true`) and restart. It is irreversible; enable deliberately. Off by default while the thread-retention semantics question is open.
- **Email-ingress messages now fire `message.posted` outgoing webhooks (LC-205).** If you run LC-75 outgoing-webhook subscribers or LC-78 bridge daemons, they now receive `message.posted` for messages that arrived via email ingress (previously these were silently not delivered). No action required unless your subscriber assumed email-ingress messages never fired webhooks.

### Added

- **Email ingress (LC-77).** Optional IMAP-polled mailbox posts mail addressed to `<token>@<ingress-domain>` into rooms. Gated on `LETS_CHAT_SECRET_KEY` set + `imap_inbox_config.enabled` + an `ingress_domain`; enabling requires a restart. See `docs/email-ingress.md`.
- **Per-message notification emails + reply-by-email (LC-77-REPLY).** Per-user opt-in (`notify_email_activity_enabled`, default off); replies via a `reply-<token>@<ingress-domain>` address post as the real user. Requires SMTP + ingress configured.
- **Bridge-avatar cache admin diagnostic page (LC-207).** Read-only `/admin/bridges/avatars` shows cache stats and recent failed avatar fetches with reasons, so "why is this bridged user showing initials" is answerable without SQL. No action required.
