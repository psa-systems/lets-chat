# Changelog

Operator-facing record of changes that affect how you **run, configure, secure, or upgrade** lets-chat. If you operate a deployment, read the Security and Changed entries before upgrading: they call out default-on behavior changes and "set this env var / upgrade promptly" actions.

This file records **tagged releases**. The project's release flow (`just create-release`, see `docs/releasing.md`) bumps the version, tags, and publishes; this file is curated at that point by the release-cutter, who reads the PR titles and descriptions merged since the last tag and picks out the operator-visible ones. Between releases, the operator-visible delta is always reconstructable from `git log --oneline --first-parent <last-tag>..HEAD` and never lives only here.

Format loosely follows [Keep a Changelog](https://keepachangelog.com). Sections: **Security** (must-act), **Changed** (behavior/default/config changes), **Added**, **Fixed**, **Deprecated**. Internal-only work (refactors, test hygiene, decoder hardening with no operator impact) is intentionally omitted; the git history is the complete record.

## [Unreleased]

## [v0.8.0] - 2026-10-20

Eighth tagged release, on top of v0.7.0. All three changes are user-facing; none needs operator action.

### Added

- **"Wave to say hi" plays a big wave animation (LC-1066).** Clicking the enclave-welcome wave now floats a large waving 👋 over the room for everyone viewing (the waver included), on top of the existing 👋 reaction. Respects reduced-motion.
- **Active voice rooms show a live participant count in the sidebar (LC-1068).** A voice channel with people on the call now carries an in-call badge, Discord-style, updated live as people join and leave, so you can see which call is active without opening it.

### Fixed

- **Add room creates the room in the enclave you are viewing (LC-1067).** After navigating between enclaves, the add-room dialog could post to a stale enclave, creating the room in the wrong place (e.g. General) and jumping you there. It now targets the current enclave.

## [v0.7.0] - 2026-10-20

Seventh tagged release, cut from `main` on top of v0.6.0. One entry is operator-facing: the GIF picker can now be configured from the admin panel instead of only a startup env var (LC-1065).

### Changed

- **An incoming 1-1 call auto-declines while you are already in a voice call (LC-1064).** In an enclave voice channel or huddle, a direct call no longer interrupts with the full-screen ring: the caller is told you are in a call and you get a quiet missed-call toast instead. An idle recipient still rings normally.

### Added

- **Configure the Giphy API key from Admin > Settings (LC-1065).** A GIF picker card sets or rotates the key at runtime, sealed at rest under `LETS_CHAT_SECRET_KEY`, taking effect with no redeploy. A stored key overrides the `LETS_CHAT_GIPHY_API_KEY` env var; with no key set the GIF button stays hidden.
- **1-1 call reactions, sounds, and a call-duration summary (LC-1063).** Floating emoji reactions in a direct call, a ringtone on an incoming call, join/leave cues in voice rooms, and a short duration panel when a call ends. Every sound follows the per-user notification-sound preference.
- **"Add chat" on the Rooms header (LC-1060).**

### Fixed

- **The sidebar no longer vanishes on enclave category changes (LC-1060).**
- **Seen-receipt timestamps show the full time on hover (LC-1062).**

## [v0.6.0] - 2026-10-20

Sixth tagged release, cut from `main` after 4 merged PRs on top of v0.5.0 (`git log --first-parent v0.5.0..v0.6.0`). One entry needs operator attention: transcription is now off after upgrade until a site admin enables it (LC-1056).

### Changed

- **Transcription is OFF after upgrade until a site admin enables it (LC-1056).** Live call transcription and voice-message / clip auto-transcripts are now behind a feature flag. **Action:** to keep transcription, a site admin sets it to Admins or Everyone on Admin > Feature flags. The AI and remote-control switches moved from Admin > Settings to that tab; their stored values are unchanged.
- **Presence is Discord-style (LC-1057).** Users go idle after 10 minutes without input (was 30), a closed or silent tab shows offline within about 90 seconds, and a reconnect no longer counts as activity. Status "Clear after" now resets the whole status (Away / Do not disturb back to Online, custom text cleared) and a running timer is kept when the status is saved again.

### Added

- **Admin > Feature flags tab (LC-1056).** Off / Admins only / Everyone rollouts for AI features, remote control and transcription.
- **Welcome message when a user joins an enclave (LC-1059).** A friendly system line posts in the enclave's welcome channel, with a "Wave to say hi" button for other members. On by default; enclave managers pick the channel or turn it off on enclave settings > General. Re-joins, startup General backfill and bot creation do not post.
- **Floating call controls (LC-1058).** The popped-out call panel has Back to call (navigates to the call's room without dropping it), Expand and Minimize buttons, on huddles and voice channels.

### Fixed

- **"Seen" times show in the viewer's local time instead of UTC (LC-1057).**
- **The status picker's Clear after dropdown works without custom text (LC-1057).**
- **"Bring back" on the floating call works from any page (LC-1058).**

## [v0.5.0] - 2026-10-19

Fifth tagged release, cut from `main` after 11 merged PRs on top of v0.4.1 (`git log --first-parent v0.4.1..v0.5.0`). No operator action is required; the entries below are the user-visible changes. Internal work (broadcast render performance LC-780 / LC-783, release CI LC-1045 / LC-1047 / LC-1049, `common` submodule bumps) is omitted and lives in the git history.

### Added

- **Settings shows the linked Bunyip account (LC-1050).** A "Bunyip account" card lists the account's SSO subject and whether its email is verified, so users and admins can tell which identity a local account is tied to. No action required.

### Changed

- **Avatar initials come from the display name, not the handle (LC-1050).** Default avatars and actor badges now use the first letter of the user's display label. Handle collisions on SSO provisioning and admin setup now get a `{handle}_{n}` suffix (for example `admin_2`). Existing handles are unchanged.
- **The sidebar Navigation block shows only on Home and global pages (LC-1051).** Inside an enclave the sidebar shows that enclave's rooms only, and the room list keeps its scroll position across live sidebar refreshes.
- **The sidebar room-list controls are one labeled toolbar (LC-1054).** "Unread only" is a labeled pill toggle beside a mark-all-read button, every sidebar block shares one gutter, the active-room highlight spans the full row, and the duplicate ROOMS-header add button is gone. On desktop a starred room's star now shows on hover with the other row controls.

### Fixed

- **Mention autocomplete no longer suggests blocked users or users with a private profile (LC-1050).**
- **The settings, enclave settings and room info pages no longer collapse to a narrow column after in-app navigation (LC-1051).**

## [v0.4.1] - 2026-10-04

No operator-visible changes; this release contains only internal CI tooling (LC-1039).

## [v0.4.0] - 2026-10-03

Fourth tagged release, cut from `main` after ~76 commits (about two weeks) of work on top of the v0.3.0 baseline. The entries below are curated from the PR titles and descriptions merged since v0.3.0 (`git log --first-parent v0.3.0..v0.4.0`); internal work (refactors, test hygiene, CI changes with no operator impact) is intentionally omitted and lives in the git history.

### Changed

- **The desktop self-updater now pulls its binary from an OCI registry, authenticated as the signed-in user (LC-733).** Let's Chat binaries are membership-gated, so the previous anonymous fetch of the Generic Packages URL could only ever answer 401. The updater now resolves `{registry}/v2/{repository}/manifests/latest-{platform}` and downloads the single artifact blob it names, using a registry credential the server hands the app after a Bunyip sign-in (`GET /desktop/registry-token`); there is no second sign-in and nothing to paste. The artifact's SHA-256 is still verified before the in-place replace, and the bearer is dropped on any cross-origin redirect. **Action:** the release still uploads binaries and `latest.json` to Forgejo Generic Packages for hand downloads, and since LC-831 it also pushes each binary to the container registry as an OCI artifact tagged `latest-{os}-x86_64` (plus a `{version}-{os}-x86_64` rollback tag), which is what the updater resolves; the registry must serve those artifacts over the OCI distribution API and accept a signed-in user's token. Operators mirroring releases replace `LETS_CHAT_UPDATE_URL` with `LETS_CHAT_UPDATE_REGISTRY_URL` (plus optional `LETS_CHAT_UPDATE_REPOSITORY` / `_TAG` / `_TOKEN`); the old variable is no longer read.

- **Desktop update manifests are no longer Ed25519-signed (LC-709).** The updater's manifest signature, the detached signature artifact published beside `latest.json`, and the public key embedded in the desktop binary are all removed. Desktop distribution is membership-gated and authenticated rather than public, so the signature is not what makes a download trustworthy; each artifact's SHA-256 is still recorded in `latest.json` and still checked before the in-place replace, which covers a corrupt or truncated download and a manifest that has drifted from the binaries it names, but not an attacker who controls the source. **Action:** the update-signing secret and public-key variable that the v0.1.0 Security entry below told you to provision are no longer read by anything; delete them from the org if you set them. Cutting a desktop release now needs no key material beyond the packages PAT.

## [v0.3.0] - 2026-08-14

Third tagged release, cut from `main` after ~198 commits (about three weeks) of work on top of the v0.2.0 baseline. The entries below are curated from the PR titles and descriptions merged since v0.2.0 (`git log --first-parent v0.2.0..v0.3.0`); internal work (UI polish, refactors, test hygiene, CI changes with no operator impact) is intentionally omitted and lives in the git history.

### Security

- **`GET /messages/{id}` now checks room membership before returning the message (LC-636).** The single-message fetch looked the message up by id alone, so any authenticated user who knew or guessed a message id could read it regardless of room membership. It now runs the same room-access check as the room message list. **Upgrade promptly.**
- **Public-room WebSocket fan-out and subscribe are now scoped to the enclave, not the whole server (LC-637).** Public-room chat events broadcast to every connected socket, and the `Subscribe` frame accepted any room id, so a signed-in user could subscribe to a public room outside their enclave and receive its message bodies. Both paths now check enclave membership. **Upgrade promptly** if you run more than one enclave.
- **SSO account adoption can no longer hijack an already-claimed identity (LC-618, tightened by LC-698).** LC-618 let a Bunyip sign-in re-adopt a local account whose `bunyip_sub` had rotated (an OP reseed), but in doing so it broadened the match past the original "unlinked row only" guard. LC-698 restores that guard and adds an explicit identity-conflict path (a bot row, an unverified-email match, or a row already linked to a different subject) that surfaces an actionable "contact an administrator" error instead of adopting the row or 500ing. No action required; a rotated-sub sign-in after an OP reseed still resolves correctly.

### Changed

- **The whole AI/LLM surface is now gated behind a runtime, role-scoped flag (LC-679).** Setting `LETS_CHAT_LLM_URL` (or the embeddings/vision/STT endpoints) only makes the backing service available; the AI surface itself stays off until a site admin flips it on in Admin > Settings > "AI features" (default off, no restart to toggle). Once on, a "Who can use AI" audience setting (LC-702, default **Everyone**) controls whether it's exposed to every user or scoped down to site admins, enclave owners/admins, and room moderators. **Action:** if you already had an LLM endpoint configured, AI features are OFF by default after this upgrade until an admin enables the flag.
- **Every boot backfills pre-existing users into the "General" default enclave (LC-621).** lets-chat now consolidates on General as the default enclave; the SSO signup path auto-joins new users, and an idempotent startup backfill (`INSERT OR IGNORE`) joins every account that existed before this upgrade. No action required.
- **Liveness/readiness probes replace the old container healthcheck (LC-581).** `GET /healthz` (dependency-free liveness) and `GET /readyz` (pings the auth/chat/settings SQLite pools, reports SSO-client presence, `503` when a store is down) are new, unauthenticated, and exempt from maintenance mode. **Action:** the container `HEALTHCHECK` now targets `/healthz` instead of the old check; point any external liveness gate at `/healthz` and readiness/load-balancer draining at `/readyz`, never the other way around, or a transient dependency blip restarts an otherwise-healthy process.

### Added

- **Image alt-text auto-draft via an operator vision model (LC-667).** Set `LETS_CHAT_VISION_URL` (an OpenAI-vision-compatible endpoint, e.g. llava/llama3.2-vision) plus optional `LETS_CHAT_VISION_API_KEY` / `LETS_CHAT_VISION_MODEL` (default `llava`) to add an "Auto-draft" button to the image alt-text editor. Separate endpoint from the chat LLM; the image is sent only to this operator endpoint. Unset = the button stays hidden.
- **Local AI moderation triage (LC-670).** With `LETS_CHAT_AI_MODERATION` set truthy and an LLM configured, posted messages are classified in the background and clear spam/harassment/inappropriate cases are flagged to the admin report queue for human review; nothing is auto-deleted or hidden. Content only reaches your own LLM.
- **Personal weekly recap DM (LC-671).** With `LETS_CHAT_WEEKLY_RECAP` set truthy and an LLM configured, each active user is DMed a short AI-written weekly recap from the assistant bot once a week; quiet weeks are skipped.
- **Background embeddings backfill for pre-existing messages (LC-673).** A new dispatcher embeds a small batch of un-embedded messages every 60s so semantic search and "Find related" cover history, not just messages posted after the embeddings endpoint was configured. No-op with no embeddings endpoint set; no action required.
- A cluster of AI-assistant features layered on the existing `LETS_CHAT_LLM_URL` / `LETS_CHAT_STT_URL` endpoints: per-bot AI persona (LC-695), admin-picked assistant-bot identity (LC-693), a translation target-language picker (LC-688), an AI writing assistant (LC-655), auto thread titles (LC-668), a scheduled per-room AI digest (LC-665), an AI gist subject for the email digest (LC-672), and call-transcript recaps (LC-662, LC-663, LC-664, LC-629). No action required beyond the LC-679 flag above; each is additional load/cost on whichever LLM/STT endpoint you have configured.

### Fixed

- **Bunyip-verified users are now marked email-verified (LC-627).** SSO provisioning never stamped `email_verified_at`, which silently blocked call remote-control (gated on a verified email) for every SSO-only user. No action required.

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
