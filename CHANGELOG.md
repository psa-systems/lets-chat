# Changelog

Operator-facing record of changes that affect how you **run, configure, secure, or upgrade** lets-chat. If you operate a deployment, read the Security and Changed entries before upgrading: they call out default-on behavior changes and "set this env var / upgrade promptly" actions.

This file records **tagged releases**. The project's release flow (`just create-release`, see `docs/releasing.md`) bumps the version, tags, and publishes; this file is curated at that point from the operator-action markers in git history. Between releases, the operator-action delta is always reconstructable from git and never lives only here:

```
git log --grep='\[operator-action\]' <last-tag>..HEAD
```

Format loosely follows [Keep a Changelog](https://keepachangelog.com). Sections: **Security** (must-act), **Changed** (behavior/default/config changes), **Added**, **Fixed**, **Deprecated**. Internal-only work (refactors, test hygiene, decoder hardening with no operator impact) is intentionally omitted; the git history is the complete record.

## [Unreleased]

### Changed

- **The desktop self-updater now pulls its binary from an OCI registry, authenticated as the signed-in user (LC-733).** Let's Chat binaries are membership-gated, so the previous anonymous fetch of the Generic Packages URL could only ever answer 401. The updater now resolves `{registry}/v2/{repository}/manifests/latest-{platform}` and downloads the single artifact blob it names, using a registry credential the server hands the app after a Bunyip sign-in (`GET /desktop/registry-token`); there is no second sign-in and nothing to paste. The artifact's SHA-256 is still verified before the in-place replace, and the bearer is dropped on any cross-origin redirect. **Action:** the release still uploads binaries and `latest.json` to Forgejo Generic Packages for hand downloads, and since LC-831 it also pushes each binary to the container registry as an OCI artifact tagged `latest-{os}-x86_64` (plus a `{version}-{os}-x86_64` rollback tag), which is what the updater resolves; the registry must serve those artifacts over the OCI distribution API and accept a signed-in user's token. Operators mirroring releases replace `LETS_CHAT_UPDATE_URL` with `LETS_CHAT_UPDATE_REGISTRY_URL` (plus optional `LETS_CHAT_UPDATE_REPOSITORY` / `_TAG` / `_TOKEN`); the old variable is no longer read.

- **Desktop update manifests are no longer Ed25519-signed (LC-709).** The updater's manifest signature, the detached signature artifact published beside `latest.json`, and the public key embedded in the desktop binary are all removed. Desktop distribution is membership-gated and authenticated rather than public, so the signature is not what makes a download trustworthy; each artifact's SHA-256 is still recorded in `latest.json` and still checked before the in-place replace, which covers a corrupt or truncated download and a manifest that has drifted from the binaries it names, but not an attacker who controls the source. **Action:** the update-signing secret and public-key variable that the v0.1.0 Security entry below told you to provision are no longer read by anything; delete them from the org if you set them. Cutting a desktop release now needs no key material beyond the packages PAT.

## [v0.2.0] - 2026-07-22

Second tagged release, cut from `main` after ~508 commits (roughly seven weeks) of work on top of the v0.1.0 seed. Server deployments track the `latest` OCI image built off `main`, so a running server does not pick this up until its image is repulled and the container is restarted; the tag is the pinning + desktop-updater baseline. The entries below are curated from the `[operator-action]` markers in `git log v0.1.0..HEAD`; internal work (refactors, test hygiene, dependency hardening with no operator impact) is intentionally omitted and lives in the git history.

### Security

- **Session tokens are now hashed at rest (LC-514).** The `sessions` table stored bearer cookies in plaintext; they are now stored hashed, and the first startup after this deploy runs a one-shot in-place re-hash of every existing row. Existing user sessions stay valid because the cookie value is the input to the new hashed lookup, so no one is logged out. **Action:** a database backup taken BEFORE this deploy still contains the plaintext cookies it captured; if that backup's cookie window has not expired (default cookie TTL is 30 days), rotate or invalidate it after deploying.
- **Security response headers are now emitted on every response (LC-504).** lets-chat now sends HSTS (`max-age=31536000; includeSubDomains; preload`), Content-Security-Policy, `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, Referrer-Policy and Permissions-Policy on every response. **Action:** if you terminate TLS at a fronting proxy that also injects these headers, reconcile them to avoid duplicates, and be aware the HSTS `includeSubDomains; preload` directive now applies to the whole domain for a year.

### Changed

- **GIF picker migrated from Tenor to Giphy (LC-505).** Google's Tenor v2 API shut down, so the composer GIF picker now uses Giphy. **Action:** set `LETS_CHAT_GIPHY_API_KEY` in place of the old `LETS_CHAT_TENOR_API_KEY`; optional `LETS_CHAT_GIPHY_RATING` (`g`/`pg`/`pg-13`/`r`, default `pg-13`). If you never set the Tenor key the picker was hidden and stays hidden until you set the Giphy key.
- **Recorded video clips are now transcribed via the STT endpoint (LC-496).** When `LETS_CHAT_STT_URL` is configured, video clips are sent to it in addition to call audio. No action required; this is a load/cost consideration on a metered or third-party STT engine, and an engine that rejects video containers simply yields no clip transcript (the clip still posts and plays).
- **Per-message translation sends message text to the LLM endpoint (LC-486).** With `LETS_CHAT_LLM_URL` set, a per-message Translate action now sends that message's text to the endpoint (cached). No action required; load/cost/content consideration on metered or third-party engines.
- **Catch-me-up summaries send chat text to the LLM endpoint (LC-484).** With `LETS_CHAT_LLM_URL` set, room and thread message text is now sent to that endpoint for on-demand summaries (cached). No action required; same consideration as above.
- **Voice messages are now transcribed via the STT endpoint (LC-483).** With `LETS_CHAT_STT_URL` set, voice-message audio is sent to it in addition to call audio. No action required; load/cost consideration on a metered STT engine.

### Added

- **LiveKit SFU stage audio (LC-512).** Optional large-audience stage audio backed by a self-hosted LiveKit server. **Action to enable:** run LiveKit and set `LETS_CHAT_LIVEKIT_URL` (`wss://...`), `LETS_CHAT_LIVEKIT_API_KEY`, `LETS_CHAT_LIVEKIT_API_SECRET`; the build must run `just vendor-js` to fetch the browser SDK (already wired into `just build` / `build-saas`). Left unset, stage roles and request-to-speak still work with no audio.
- **GIF picker in the composer (LC-488, superseded by LC-505).** Introduced against Tenor and migrated to Giphy within this release window; see the Giphy entry under Changed for the current env var.

### Deprecated

- **`LETS_CHAT_TENOR_*` env vars (LC-505).** Replaced by `LETS_CHAT_GIPHY_*` after the Tenor API shutdown. The old vars are ignored.

## [v0.1.0] - 2026-06-24

First tagged release. lets-chat previously shipped only as the `latest` OCI image built off `main`; v0.1.0 is the seed version cut to a tag so deployments can pin a release (e.g. `lets-chat:v0.1.0` in your OCI registry) and the desktop self-updater has a baseline. This entry is the operator-visible snapshot of everything in `main` at the tag, folding the prior **Pre-release** seed (snapshot 2026-05-30) together with the operator-action changes that landed after it.

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
