# Troubleshooting

Concrete operator failures with the underlying cause and the fix.

## 1. Sign-in loops back to the sign-in page

Symptom: a user clicks sign in, lands on Bunyip, approves, returns to lets chat, and immediately sees the sign-in page again.

Likely causes:
- The configured `LETS_CHAT_BUNYIP_SSO_REDIRECT_URI` does not match the URI registered on the Bunyip client, character for character (trailing slash, http vs https, port).
- The `LETS_CHAT_PUBLIC_URL` differs from the hostname the browser used, so the session cookie is set on a different host than the one the user is on.
- A reverse proxy strips or rewrites the session cookie.

Fix: verify the four SSO variables exactly match the Bunyip client registration. Verify the cookie lands on the right host (browser dev tools, Application tab). If a reverse proxy is in the path, verify it forwards `Set-Cookie` headers from the server unmodified.

## 2. Desktop update verification fails

Symptom: the desktop app finds a new version, downloads it, then errors "signature verification failed" and refuses to install.

Likely causes:
- The release manifest was signed with a different private key than the one whose public half is baked into the installed binary.
- The manifest file was edited after signing (even a whitespace change breaks the signature).
- The private signing key was rotated without first shipping a bridge release signed with the old key (see [Desktop update signing](operations/desktop-update-signing.md)).

Fix: re-sign the manifest with the exact key whose public half is in the field, and publish the re-signed version. If you rotated the key by accident, ship a new binary out-of-band that embeds the new public key.

## 3. Email ingress drops messages silently

Symptom: mail sent to a room token address does not appear in the room. The admin audit page shows no delivery.

Likely causes:
- IMAP credentials rotated without updating `LETS_CHAT_EMAIL_INGRESS_IMAP_PASSWORD`. The poll loop logs a warning and skips.
- The ingress mailbox hit its quota. New mail is rejected on the mail provider side; lets chat never sees it.
- A spam filter on the mail provider quarantined the inbound mail before lets chat polled.
- The room's ingress token was rotated and the sender is still using the old address.

Fix: tail the server log for the `email_ingress` module and look for the most recent poll outcome. Verify the mail actually reached the mailbox (sign in to the mailbox directly). Rotate credentials if needed and restart.

## 4. 401 Unauthorized across every API route

Symptom: every `/v1/*` request returns 401. The user can still sign in through the browser.

Likely causes:
- The user is calling the API with a browser cookie. The API accepts API tokens, not cookies.
- The API token was revoked. Revocation is immediate.
- The token's scope does not cover the route the user called.

Fix: confirm the caller is sending `Authorization: Bearer <token>` and that the token is still active in `/settings/api-tokens`. If the caller is a bot, mint a token with the right scope and update the bot's secret.

## 5. Protocol bridge keeps reconnecting

Symptom: the admin page shows the bridge flipping between healthy and unhealthy every few minutes; nothing posts from the bridge side.

Likely causes:
- The daemon's self-filter is not detecting its own round-trips, so it keeps sending events the server deduplicates. Eventually its outbound channel stalls and the daemon reconnects.
- The webhook target URL became unreachable but the daemon still heartbeats.
- The bridge registration's scoped API token was rotated and the daemon still uses the old one.

Fix: check `bridge_loops_prevented` in the admin metrics panel. A rising number means the daemon is not self-filtering; patch the daemon. Hit the webhook URL from the host lets chat runs on and confirm it responds. Rotate the bridge token in `/admin/bridges` and update the daemon.
