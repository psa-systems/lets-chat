# Changelog

Operator-facing record of changes that affect how you **run, configure, secure, or upgrade** lets-chat. If you operate a deployment, read the Security and Changed entries before upgrading: they call out default-on behavior changes and "set this env var / upgrade promptly" actions.

This file records **tagged releases**. The project's release flow (`just create-release`, see `docs/releasing.md`) bumps the version, tags, and publishes; this file is curated at that point from the operator-action markers in git history. Between releases, the operator-action delta is always reconstructable from git and never lives only here:

```
git log --grep='\[operator-action\]' <last-tag>..HEAD
```

Format loosely follows [Keep a Changelog](https://keepachangelog.com). Sections: **Security** (must-act), **Changed** (behavior/default/config changes), **Added**, **Fixed**, **Deprecated**. Internal-only work (refactors, test hygiene, decoder hardening with no operator impact) is intentionally omitted; the git history is the complete record.

## [v0.1.0] - 2026-06-24

First tagged release. lets-chat previously shipped only as the `latest` OCI image built off `main`; v0.1.0 is the seed version cut to a tag so deployments can pin a release (`dev.a8n.run/a8n-tools-private/lets-chat:v0.1.0`) and the desktop self-updater has a baseline. This entry is the operator-visible snapshot of everything in `main` at the tag, folding the prior **Pre-release** seed (snapshot 2026-05-30) together with the operator-action changes that landed after it.

### Security

- **Authentication is Bunyip SSO only; local auth retired (LC-22).** The standalone build refuses to start without all four `LETS_CHAT_BUNYIP_SSO_ISSUER` / `_CLIENT_ID` / `_CLIENT_SECRET` / `_REDIRECT_URI` vars set and the OP reachable for discovery + JWKS; there is no local username/password, registration, password reset, or 2FA. **Action:** land the bunyip-api `oauth_clients` seed migration first, set the four vars on every deployment, snapshot `auth.db` before deploy (the cutover migration is irreversible), then deploy. Existing local users cannot sign in post-cutover; re-graft a returning user's authored content manually once they come back via Bunyip.
- **Unified outbound SSRF guard; closed an unguarded Web Push SSRF (LC-152).** All server-initiated outbound HTTP (outgoing webhooks, Web Push, bridge-avatar fetch) now routes through a single guarded client that refuses connections resolving to private / non-public addresses. The audit that motivated this found Web Push (`push/mod.rs`) had **no SSRF guard at all** in shipped versions, so a crafted push endpoint could reach internal-network addresses and exfiltrate response metadata. **Upgrade promptly** if your deployment has Web Push or outgoing webhooks enabled.
- **Bounded image decode closes an unbounded GIF decompression-bomb DoS (LC-206-IMAGE-LIMITS).** Shipped versions decoded GIFs through an unbounded decoder; an attacker-supplied GIF bomb (small file, huge decoded size) via the foreign-avatar fetch or upload pipeline could exhaust server memory and crash the process (remote DoS). No configuration change is required; the fix is the explicit decode `Limits`. **Upgrade promptly.**
- **Signed desktop update manifest + binary-hash verification (LC-210-BINARY-INTEGRITY).** The desktop updater now verifies an Ed25519-signed `latest.json` and each binary's SHA-256 before the in-place replace, and fails closed if unverified. **Action (before cutting desktop releases):** provision update signing - generate an Ed25519 keypair, set the `DESKTOP_UPDATE_SIGNING_KEY` secret (private PEM) and `DESKTOP_UPDATE_PUBLIC_KEY` variable (public key hex) per `docs/desktop-update-signing.md`. Vector: an unverified binary served via update-mirror compromise or a redirect to a public attacker host.

### Changed

- **Foreign bridge-avatar proxy fetching is default-ON (LC-78-AVATAR-PROXY).** When a protocol bridge submits a foreign avatar URL, the server now fetches it server-side (capped, SSRF-guarded, re-encoded) and serves it same-origin, instead of rejecting it. This means **outbound fetches to foreign homeservers happen by default** on bridge traffic. To restore the v1 reject-non-null behavior, set `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false` (or `0`).
- **Destructive message-retention sweep, gated default-OFF (`LETS_CHAT_RETENTION_SWEEP_ENABLED`).** A background sweep that **hard-deletes** messages past a room's `retention_days` is shipped but disabled unless you set `LETS_CHAT_RETENTION_SWEEP_ENABLED=1` (or `true`) and restart. It is irreversible; enable deliberately. Off by default while the thread-retention semantics question is open.
- **Email-ingress messages now fire `message.posted` outgoing webhooks (LC-205).** If you run LC-75 outgoing-webhook subscribers or LC-78 bridge daemons, they now receive `message.posted` for messages that arrived via email ingress (previously these were silently not delivered). No action required unless your subscriber assumed email-ingress messages never fired webhooks.

### Added

- **Email ingress (LC-77).** Optional IMAP-polled mailbox posts mail addressed to `<token>@<ingress-domain>` into rooms. Gated on `LETS_CHAT_SECRET_KEY` set + `imap_inbox_config.enabled` + an `ingress_domain`; enabling requires a restart. See `docs/email-ingress.md`.
- **Per-message notification emails + reply-by-email (LC-77-REPLY).** Per-user opt-in (`notify_email_activity_enabled`, default off); replies via a `reply-<token>@<ingress-domain>` address post as the real user. Requires SMTP + ingress configured.
- **Bridge-avatar cache admin diagnostic page (LC-207).** Read-only `/admin/bridges/avatars` shows cache stats and recent failed avatar fetches with reasons, so "why is this bridged user showing initials" is answerable without SQL. No action required.
- **Optional self-hosted server-side call transcription (LC-393).** Set `LETS_CHAT_STT_URL` (+ optional `_API_KEY` / `_MODEL`) to an OpenAI-compatible `/v1/audio/transcriptions` endpoint to switch call transcription from the in-browser Web Speech engine to self-hosted STT (browser-agnostic, keeps audio off third-party clouds). Leave unset to keep the browser engine.
- **Optional AI transcript summaries (LC-396).** Set `LETS_CHAT_LLM_URL` (+ optional `_API_KEY` / `_MODEL`) to an OpenAI-compatible `/v1/chat/completions` endpoint to enable the transcript "Summarize" action (markdown summary + action items, cached). Leave unset to hide the action.
