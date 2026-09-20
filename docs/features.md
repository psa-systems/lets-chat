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
- Multi-party enclave voice channels
- Per-room stage mode: speaker roles and request-to-speak, with audio carried by a self-hosted LiveKit SFU when `LETS_CHAT_LIVEKIT_*` is configured (the mesh path caps at a handful of peers)
- Call transcription, either in the speaker's browser (Web Speech) or server-side against a transcription endpoint you run
- Consent-gated remote control: during a 1:1 call, request keyboard/mouse control of a peer's screen (desktop app; verified-email gated, revocable at any time). Off by default; an admin must enable it at `/admin/remote-control` before users can request control.

## Spaces and organization

- Enclaves: grouped rooms/workspaces, each with a default room and settings gear
- Room and DM mute, sidebar categories, starred rooms, and user groups
- Custom user status

## Personalization

- Light, dark, and high-contrast (WCAG AA/AAA) themes, plus a "follow system" option; saved per user at `/settings`
- Comfortable or compact display density
- Localized UI with an in-app language picker (English and Spanish), falling back to the browser language (see [i18n.md](i18n.md))

## Notifications

- Web push notifications
- Email digest of missed mentions and DMs, off by default per user (see [email-digests.md](email-digests.md))
- Per-mention and per-DM notification emails (off by default per user); reply to one of those emails to post that reply to chat as yourself

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

## Administration

- Admin panel: user management, room management, settings
- Analytics dashboard: DAU/MAU, messages, rooms, signups, retention
- Branding: custom logo, colors, login text, and favicon
- Anti-spam: rate limits, link filter, and honeypot
- Backup and restore archive
- Moderator tools: mute, ban, kick, delete messages
- Single sign-on: "Sign in with Bunyip" (OIDC) is the sole authentication path (LC-22); there is no local username/password, registration, password reset, or 2FA
- New-country sign-in alerts, and an optional notify-and-approve gate for suspicious logins
- Installable PWA with an offline message outbox
- Role-based access: Admin > Moderator > User
