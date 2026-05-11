# TODO

Feature ideas not yet built. SSO tracked separately.

## Auth / account

- Password reset flow (email-based)
- Email verification on registration
- Multi-device session list with per-session revoke (sessions table exists; no UI)
- Account data export (GDPR-style dump)
- Self-serve account deletion
- Login alerts / new-device email

## Messaging power-ups

- Markdown rendering (bold, italic, lists, blockquote)
- Code fence syntax highlighting
- LaTeX / math rendering
- Quote-reply (distinct from threads) and forward / cross-post
- Edit history view (currently only an "edited" badge)
- Scheduled send ("send at 9am")
- Reminders ("remind me about this message")
- Saved messages / bookmarks (personal channel)
- Server-persisted drafts (cross-device)
- Voice messages (record + waveform)
- Polls / voting
- Ephemeral / self-destruct messages
- Per-room retention policy (auto-delete after N days)

## Real-time

- Voice/video calls (WebRTC, start with 1:1)
- Screen share
- Richer presence (online / away / DND beyond status text)

## Integrations / extensibility

- Public REST API with scoped tokens
- Bots
- Incoming webhooks (post-as URL)
- Outgoing webhooks / event subscriptions
- Slash commands (`/remind`, `/poll`, `/giphy`, custom)
- Email-to-room gateway (reply by email)
- Bridges: Matrix, IRC, XMPP

## Organization / navigation

- Channel categories with collapsible sections
- Favorite / starred rooms
- Jump-to-unread and unread inbox view
- Activity center (all mentions and reactions across rooms)
- User groups / `@team-foo` mentionable subgroups
- Per-room role overrides (granular permissions)
- Read-only announcement rooms
- Room topic / description / pinned docs / wiki tab
- Per-room file browser and media gallery

## Notifications

- DND / quiet hours / schedule
- Email digest for missed mentions
- Per-room notification level (all / mentions / none) — verify status
- Mobile push (iOS APNs, Android FCM) separate from Web Push

## Admin / ops

- Maintenance mode UI toggle (SaaS has webhook; standalone has none)
- Storage quota per user and per enclave
- Anti-spam: rate limits, link filter, CAPTCHA on register
- Backup / restore from admin UI
- Branding: custom logo, primary color, login page text
- Analytics dashboard (DAU, messages/day, active rooms)

## Client surface

- PWA install + offline outbox
- Mobile apps (Tauri mobile or Capacitor wrapper)
- i18n / localization
- Accessibility audit (keyboard nav, ARIA, focus rings)
- RSS / iCal feed per room
