# Email ingress

Lets chat polls an IMAP mailbox every five minutes. Mail addressed to `<token>@<ingress-domain>` posts to the target room. Reply-by-email turns notification emails into chat messages from the real user.

## What this is

- **Room ingress**: an external sender emails a per-room token address. The message appears in the room as a synthetic `email` actor with the sender's address in a badge. No real-user impersonation; no DM link.
- **Reply-by-email**: notification emails to mentioned users carry a reply-token. When the user replies, the IMAP poll loop consumes the reply and posts it to the referenced message's room as the real user.

Notification emails themselves are the outbound side; the IMAP loop only handles inbound replies and new ingress.

## Mailbox

One mailbox for the whole deployment. Lets chat polls `INBOX`, processes each new message, and moves the processed message to a `Processed/` folder on the IMAP side (so reprocessing on crash does not duplicate).

Create a mailbox for this on your mail server. It needs:
- IMAP enabled, TLS on, standard authentication.
- A folder `Processed` (lets chat creates it on first use if the IMAP server allows).
- A sensible mailbox quota; a backlog of hundreds of unprocessed emails behaves badly in the loop timings.

## Configuration

Set these variables (see [Configuration](../configuration.md)):

- `LETS_CHAT_EMAIL_INGRESS_IMAP_HOST`
- `LETS_CHAT_EMAIL_INGRESS_IMAP_USER`
- `LETS_CHAT_EMAIL_INGRESS_IMAP_PASSWORD`
- `LETS_CHAT_EMAIL_INGRESS_DOMAIN`

A missing variable disables the subsystem; present-but-wrong values cause the next poll to log the failure and skip until the values are fixed.

## DNS

Your mail must land in the configured mailbox. Point MX or an alias at the mailbox provider; whatever the mail provider accepts.

If you use a catchall on a domain, use a subdomain just for ingress (`chat.example.com` for the app, `in.chat.example.com` as the ingress domain). A catchall on the main MSP domain collides with legitimate mail; a subdomain scopes the risk.

## Tokens

Each room has a token used as the local part of the ingress address. The address format is `<token>@<ingress-domain>`. Admins mint, rotate, and revoke tokens from the room settings; a rotated token invalidates the old address immediately (future sends to the old address silently fail on the loop side).

## Reply-by-email

Notification emails carry an opaque reply-token tied to the message and the user. When a reply lands in the ingress mailbox, lets chat:

1. Matches the reply-token to a message id.
2. Looks up the user that received the notification.
3. Extracts the quoted reply body (strip the forwarded original).
4. Posts the body in the referenced room as the real user.

Replying from a shared mailbox works as long as the reply preserves the reply-token header lets chat embedded.

## Failure modes

- IMAP connection lost: the loop logs a warning and retries at the next tick. No queueing on the mail side; the mailbox buffers until lets chat catches up.
- Reply-token invalid, expired, or already consumed: the reply is dropped, logged, and the user's bounce address receives a plain note.
- Spam delivered to the ingress mailbox: lets chat does not filter spam. Put spam filtering on the mail provider side.
