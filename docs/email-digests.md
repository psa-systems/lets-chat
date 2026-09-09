# Email digests

Sends each opted-in user one email summarizing mentions and DMs they missed
while offline. Variable-by-variable SMTP reference is in
[configuration.md](configuration.md); inbound mail is in
[email-ingress.md](email-ingress.md).

## Operator setup

Three things must be configured for the feature to be fully functional.

1. **SMTP environment variables**. The same set used by all outbound mail (digest and mention/DM notifications). The four non-credential vars are all required to enable outbound mail:
   - `LETS_CHAT_SMTP_HOST`
   - `LETS_CHAT_SMTP_PORT`
   - `LETS_CHAT_SMTP_TLS` (one of `tls` / `starttls` / `none`)
   - `LETS_CHAT_SMTP_FROM`
   - `LETS_CHAT_SMTP_USERNAME` and `LETS_CHAT_SMTP_PASSWORD` together (optional pair; both unset means the relay is opened unauthenticated)
2. **`LETS_CHAT_BASE_URL`** in the environment, e.g. `https://chat.example.com`. Used to construct clickable deep links in the email body. Defaults to `http://localhost:8080` if unset; the digest still sends with that URL but the links will only work for local development.
3. **(Optional) "New users start with email digest enabled"** at `/admin/settings`. Off by default. Flipping it on only affects users who first sign in after the flip; existing users are unchanged. Users can override their own preference at `/settings`.

Changes to SMTP env vars take effect on the next server restart.

## User opt-in

1. Sign in and go to `/settings`. Your email address comes from your Bunyip account (the SSO `email` claim).
2. Tick "Email me a digest of missed mentions and DMs".
3. Save preferences.

Users with no email address on file are skipped by the digest tick regardless of the checkbox state. Muted rooms (`mute_mode = 'all'`) and muted DMs are excluded; `mute_mode = 'except_mentions'` rooms still contribute their mentions.

## Delivery semantics

- **Cadence**: hourly background tick. First fire one hour after server start.
- **Quiet period**: 1 hour. The user must have had neither HTTP activity (`last_active_at`) nor WebSocket activity (`last_ws_seen_at`) within the last hour.
- **One digest per offline session**: the tick gates on `last_digest_sent_at < MAX(last_active_at, last_ws_seen_at)`. As soon as the user comes back online and bumps either column, the gate self-resets, so the next offline session is eligible for one more email.
- **Time window**: 7 days. Activity older than 7 days never appears in a digest.
- **Item cap**: 50 across all sections combined. Overflow renders as a "... and N more" footer.
- **Subject**: `[lets-chat] N new mentions and M direct messages` (zero clauses dropped).
- **Format**: `multipart/alternative` with both `text/plain` and `text/html` parts.
