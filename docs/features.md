# Features

What the product does, surface by surface. Configuration for the optional
surfaces is in [configuration.md](configuration.md).

## Messaging

- Public chat rooms and private/invite-only rooms with real-time messaging
- Direct messages between users
- Message editing with live updates and an edit-history drawer
- Typing indicators, read receipts, and message grouping
- Emoji reactions, including custom emoji
- Full-text message search, plus optional semantic search and "Find related" when an embeddings endpoint is configured
- Pinned messages and per-user bookmarks
- Polls and voting
- Scheduled message delivery: pick a future time in the composer, see and edit pending sends at `/scheduled`
- Message reminders ("remind me about this message")
- Slash commands
- File and image uploads with link unfurling
- `@`-mentions, broadcast mentions, and a notifications inbox

## Voice and video

- 1:1 audio and video calls over WebRTC, with mic/camera/speaker selection
- 1:1 calls play a ringtone on ring, show a running duration, and support floating emoji reactions during the call
- Huddles: multi-party enclave voice channels, with a ring notification to room members who are not already viewing the room
- Per-room stage mode: speaker roles and request-to-speak, with audio carried by a self-hosted LiveKit SFU when `LETS_CHAT_LIVEKIT_*` is configured (the mesh path caps at a handful of peers)
- Call transcription, either in the speaker's browser (Web Speech) or server-side against a transcription endpoint you run
- Consent-gated remote control: during a 1:1 call, request keyboard/mouse control of a peer's screen (desktop app, Windows only; verified-email gated, revocable at any time). Off by default; an admin must enable it at `/admin/remote-control` before users can request control, and can review the consent history on a dedicated audit page.
- In-call text chat: a Chat control in the voice call bar opens a draggable panel where anyone in the call can send and read messages live during the call, posted to the voice room's own message channel
- Screen-share alongside the audio/video stream
- Active voice rooms show a live in-call participant count in the sidebar

### Linux screen sharing prerequisites

The desktop app's screen share on Linux goes through WebKitGTK's `getDisplayMedia`, which needs GStreamer and PipeWire (via `xdg-desktop-portal`) to build the capture pipeline. The `.deb` package declares these as dependencies, so a `.deb` install gets them for free. The raw binary and the `.AppImage` do not bundle them, so install them yourself first:

- Debian/Ubuntu: `gstreamer1.0-plugins-base`, `gstreamer1.0-plugins-good`, `gstreamer1.0-pipewire`, `pipewire`, `xdg-desktop-portal`
- Fedora: `gstreamer1-plugins-base`, `gstreamer1-plugins-good`, `pipewire-gstreamer`, `pipewire`, `xdg-desktop-portal`
- Arch: `gst-plugins-base`, `gst-plugins-good`, `gst-plugin-pipewire`, `pipewire`, `xdg-desktop-portal`

A desktop-specific portal backend must also be installed and running, matching your desktop environment: `xdg-desktop-portal-gnome`, `xdg-desktop-portal-kde`, `xdg-desktop-portal-gtk`, or `xdg-desktop-portal-wlr`. Without it, sharing fails and the call UI shows an error toast naming the failure.

## Spaces and organization

- Enclaves: grouped rooms/workspaces, each with a default room and settings gear
- Room wiki: a per-room markdown page, editable inline by moderators and up
- Room and DM mute, sidebar categories, starred rooms, and user groups
- Custom user status
- Presence shows idle after 10 minutes of inactivity and offline on a silent background tab, Discord-style
- New members joining an enclave post an automatic welcome message, with an optional animated wave reaction; configurable in enclave Settings > General
- Handle picker: pick and change your own `@handle`, subject to the same validation the SSO provisioning flow uses to derive one
- Private rooms get a dedicated member-add picker in the room header to invite existing enclave members

## Personalization

- Light, dark, and high-contrast (WCAG AA/AAA) themes, plus a "follow system" option; saved per user at `/settings`
- Comfortable or compact display density
- Localized UI with an in-app language picker (English and Spanish), falling back to the browser language (see [i18n.md](i18n.md))

## Notifications

- Web push notifications
- Email digest of missed mentions and DMs, off by default per user (see [email-digests.md](email-digests.md))
- Per-mention and per-DM notification emails (off by default per user); reply to one of those emails to post that reply to chat as yourself
- Running in the desktop app: a mention, DM, or incoming call raises a native OS notification instead of a browser one (same mute / do-not-disturb gating), the dock/taskbar icon shows your unread count, and `lets-chat://room/<id>` (or `/dm/...`, `#msg-...`) links open and focus the app at that destination

## Integrations and API

- JSON HTTP API v1 with scoped bearer tokens (see [api.md](api.md))
- First-class bot identities
- Incoming webhooks (post via secret URL) and outgoing webhooks (signed event subscriptions)
- Email ingress: per-room IMAP-poll inboxes that turn an `<token>@<ingress-domain>` address into chat posts (see [email-ingress.md](email-ingress.md))
- Protocol bridges for foreign networks (see [protocol-bridges.md](protocol-bridges.md))
- Per-room Atom and iCal feeds, each served from a revocable secret-token URL

## Optional AI surfaces

Every one of these is off until an operator points it at an endpoint they run, and the whole surface stays hidden until an admin also flips "AI features" in Admin > Settings. Endpoints are operator-trusted, so they may be `localhost` or an internal service. Configuration is in [configuration.md](configuration.md).

- Summary and action items for a saved call transcript
- Semantic search and "Find related" messages
- Auto-drafted image alt text
- Moderation triage: clear spam / harassment cases are flagged to the admin report queue for human review, never auto-deleted
- A weekly personal recap DM
- AI help desk: `/support <question>` answers from the indexed docs, with an escalation path to a human in the admin support queue

## Administration

- Admin panel: user management, room management, settings
- Feature flags: Off / Admins only / On rollout control for AI surfaces, remote control, server-side transcription, and in-call diagnostics (admin-only), at `/admin/feature-flags`
- Analytics dashboard: DAU/MAU, messages, rooms, signups, retention
- Branding: custom logo, colors, login text, and favicon
- Anti-spam: rate limits, link filter, and honeypot
- Backup and restore archive
- Moderator tools: mute, ban, kick, delete messages
- Single sign-on: "Sign in with Bunyip" (OIDC) is the sole authentication path (LC-22); there is no local username/password, registration, password reset, or 2FA
- New-country sign-in alerts, and an optional notify-and-approve gate for suspicious logins
- Installable PWA with an offline message outbox
- Role-based access: Admin > Moderator > User
- Voice diagnostics log: a live, admin-only feed of voice-call events for observability
- Deployment-environment badge: when `LETS_CHAT_ENVIRONMENT` names a non-production deployment, it is shown in the app shell and on the sign-in, first-entry, and invitation surfaces
