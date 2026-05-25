# Email ingress

LC-77 lets external senders mail a chat room directly. An operator-configured IMAP mailbox is polled every 5 minutes; messages addressed to `<token>@<ingress-domain>` post to their target room as a synthetic "Email" actor (no real user impersonation, no DM link, "email" badge in the rendered row).

The reply-by-email half of LC-77 is not in v1; this doc covers the per-room ingress surface only.

## What this is, and what it isn't

**Is**: an inbound bridge. An external system (monitoring alert, ticketing tool, vendor system, occasional human-typed update from a colleague who only has email) mails the inbox address, the message appears in chat.

**Isn't**: an SMTP server. lets-chat does not bind port 25 or 587 and does not own MX for any domain. The operator provides a mailbox at their mail provider; lets-chat polls it via IMAP.

**Isn't**: a reply-to-chat-notification system. Replying to chat email notifications is the deferred half of LC-77 and lives in a separate ticket.

## Threat model

The secret in the inbox address is the entire authorization boundary. Concretely:

- **Identity is the secret, not the From header.** The `From:` of an inbound email is trivially forged. lets-chat does NOT trust `From:` for identity. A message addressed to `<token>@<ingress-domain>` posts as the inbox's configured display name and avatar, regardless of who claims to have sent it. Test: `email_ingress_threat_model::forged_from_still_posts_as_inbox_actor`.

- **The token's hash is the persistence boundary.** Only `HMAC-SHA256(LETS_CHAT_SECRET_KEY, plaintext_token)` is stored, in `email_inboxes.secret_hash`. The plaintext token lives in two places: the rendered HTML on the create-response page (shown ONCE) and the operator's mailbox (where the sender mailed it). A chat.db leak cannot reconstruct usable inbox addresses.

- **Magic-byte sniffing is the attachment trust boundary.** The sender's `Content-Type:` is informational only. The pipeline streams the decoded part to a temp file, calls `infer::get_from_path`, and rejects anything not in the allowlist (jpg / png / gif / webp / pdf). A JPEG header followed by zip bytes is rejected. Test: `email_ingress_attachments::jpeg_claiming_zip_dropped_at_sniff_message_still_posts`.

- **EXIF is stripped on image upload.** Images go through the same decode + re-encode pipeline the web upload path uses. The decoder discards EXIF / XMP / IPTC / PNG text chunks and the encoder writes none of them back. Test: `email_ingress_attachments::jpeg_re_encode_strips_exif_signature_from_stored_bytes`.

- **No raw HTML in body.** HTML-only messages fall through `mail-parser`'s text fallback (or drop with `ParseFail` if no text is recoverable). The chat markdown pipeline already strips raw HTML in user input; the email path never feeds it raw HTML in the first place. Test: `email_ingress_threat_model::raw_html_never_appears_in_stored_body`.

- **Revoked inboxes drop silently.** No bounce email; the operator's only diagnostic is the structured `email_ingress::drop` log line. Revealing whether an inbox address exists via a bounce would let an attacker enumerate live inboxes.

- **Per-inbox rate limit.** 60 messages per minute per inbox. Over-limit messages drop with `reason=rate_limited`.

- **Mailbox provider is the first line of spam defense.** lets-chat does NOT ship a content-filter layer. The expectation is that the operator's mailbox provider (Fastmail, Gmail, etc.) already filters junk; whatever reaches the poll is treated as intended for the room.

## Deployment

### What the operator owns

- **An MX-receiving domain.** Mail addressed to `*@<your-ingress-domain>` must reach a mailbox the server can poll. This is the operator's responsibility; lets-chat is not in the SMTP path.
- **A mailbox at any IMAP provider.** Fastmail, Gmail (with app password), Migadu, a self-hosted mailbox, anything that speaks IMAP over TLS on port 993.
- **The `LETS_CHAT_SECRET_KEY` environment variable.** Required for sealing the IMAP password at rest (AES-256-GCM under the SHA-256 of this key) and for HMAC-hashing inbox secrets.

### Header-precedence requirement (load-bearing)

This is the operational gotcha that decides whether email ingress works on a given deployment.

The poll loop reads the secret address from one of these headers, in priority order:

1. `Delivered-To`
2. `X-Original-To`
3. `To`
4. `Cc`

**Mail must arrive at the polled mailbox with `<token>@<ingress-domain>` present in one of those headers.**

If the operator forwards mail into the polled mailbox (rather than the mailbox being the direct MX), the forwarder must preserve at least one of those headers in the forwarded message. Most forwarders set `Delivered-To` and `X-Original-To` correctly; some rewrite `To:` to the forwarding-target address, which loses the original `<token>@<ingress-domain>` but is fine as long as `Delivered-To` or `X-Original-To` still carries it.

If a polled message has the token in NONE of those headers, the resolver returns `AddressNoMatch`, the message drops, and the log line includes `tried_addresses` listing exactly which addresses were checked. That list is the operator's diagnostic when "my email didn't post."

### Setup steps

1. **Provision the mailbox.** Create a dedicated mailbox at your IMAP provider for ingress. Do not reuse a human mailbox; the poll loop marks every processed message `\Seen` and the volume can be substantial.
2. **Set `LETS_CHAT_SECRET_KEY` if you haven't.** Generate a strong random string (`head -c 32 /dev/urandom | base64`) and put it in the server's environment. Restart the server.
3. **Configure the IMAP poll.** As an admin, visit `/admin/settings` and fill in the "Email ingress (IMAP poll)" section: host, port (993 for IMAPS, recommended), TLS on, username, password (write-only; sealed via AES-256-GCM under `LETS_CHAT_SECRET_KEY`), folder to poll (usually `INBOX`), ingress domain. Check "Enable IMAP poll" and save. **Restart the server.** The spawn gate is read at startup, not per tick.
4. **Create a per-room inbox.** As a room moderator, visit `/room/{id}/email-inboxes`, fill in a display name, optionally an avatar URL, and click Create. The full `<token>@<ingress-domain>` address is shown ONCE in a green banner; copy it now. It won't be shown again.
5. **Test.** Mail something to that address. Within 5 minutes (next poll tick), it should appear in the room.

### Verifying it works

If the test message does not post within ~5-10 minutes:

1. **Check the server logs.** Filter to `target=email_ingress` for the spawn-time messages and `target=email_ingress::drop` for per-message drops. If you see `email ingress disabled: ...`, the spawn gate refused to start; the message names the missing piece.
2. **If the spawn is running but no `email_ingress` lines fire on the tick interval**: the poll connected and found 0 unseen messages. The mail did not reach the mailbox. Check at the IMAP provider.
3. **If a `target=email_ingress::drop reason=address_no_match` line fires**: the mail reached the mailbox but the resolver could not find the token. The log's `detail` field includes the tried addresses; this tells you exactly which headers the operator's MTA preserved (or didn't). Fix the forwarder, OR mail directly to the polled mailbox without an intermediate forward.
4. **Other drops** (`parse_fail`, `revoked_inbox`, `loop_detected`, `rate_limited`): the log carries `reason` + `detail` and the inbox-id + sender. The taxonomy below names what each reason means.

## Failure-log taxonomy

Every dropped message logs at `WARN` with `target: "email_ingress::drop"`. The `reason` field is one of:

| `reason` | What it means | Operator action |
|---|---|---|
| `parse_fail` | mail-parser returned no message OR the body + subject were empty and there were no attachments. | Almost always a malformed sender. Check `detail` for parser hints. |
| `address_no_match` | The token in none of Delivered-To / X-Original-To / To / Cc matched a known inbox secret. | Check `detail` for the list of tried addresses. Usually a forwarder dropped the header that carried the original recipient. |
| `revoked_inbox` | The token matched, but the inbox has been revoked by an admin. | Tell the sender to use a fresh inbox (created via the per-room admin page). |
| `loop_detected` | The message looked machine-generated (Auto-Submitted, Precedence: bulk/list/junk, X-Autoreply, X-Autorespond, or List-Id). | This is a deliberate drop. **`List-Id`-tagged mail is dropped even when the sender is a legitimate automated tool that happens to tag itself as a list** - that's the v1 posture; if your monitoring tool tags itself with `List-Id`, see the "Not supported" section below. |
| `rate_limited` | Inbox exceeded 60 messages per minute. | The sender is misbehaving; throttle at the source. |
| `internal_error` | Catch-all (DB, disk, IMAP transport). | Check `detail`; usually a transient. If persistent, investigate the lets-chat data dir or the IMAP provider. |

Attachment drops are logged separately at `INFO` with `target: "email_ingress::attachment_drop"` and `reason` ∈ {`over_size`, `disallowed_mime`, `sniff_failed`, `image_pipeline`, `io`, `db`}. The parent message still posts; the bad attachment is the only thing dropped.

## Limits

| Limit | Value | Adjustable? |
|---|---|---|
| Poll interval | 5 minutes | Hardcoded |
| Per-inbox rate | 60 messages / minute | Hardcoded |
| Raw RFC 822 size (per polled message) | 5 MiB | Hardcoded |
| Chat-body size (subject + body, truncated with `_[truncated]_` marker over) | 64 KiB | Hardcoded |
| Attachments per message | 4 | Hardcoded |
| Per-attachment size | 10 MiB | Set via admin settings `max_upload_bytes` (same setting web uploads use) |
| Attachment MIME allowlist | jpg / png / gif / webp / pdf | Hardcoded |

"Hardcoded" means: changing requires a code change + release. The admin UI does not expose these. If you need a different value, file a followup.

## Not supported (deferred)

- **Reply-by-email.** Mailing a reply to a chat notification email does not post to chat in v1. Tracked as the `LC-77-REPLY` follow-up. Depends on a per-message notification email surface that doesn't exist yet.
- **Mailing list ingestion.** Messages with `List-Id` are dropped. This is intentionally conservative: a sender that tags itself as a list is usually broadcasting, not communicating with a chat. If your monitoring tool sets `List-Id` and you NEED to ingest its mail, reconfigure the tool not to set `List-Id`, or wait for the follow-up that adds per-inbox loop-detection overrides.
- **Auto-responder ingestion.** Out-of-office replies and vacation messages drop with `loop_detected detail="Auto-Submitted: ..."`. Same posture as `List-Id`.
- **HTML email rich rendering.** HTML-only messages either drop with `parse_fail` (no text fallback recoverable) or post the stripped-to-text version. A dedicated HTML-to-Markdown converter is a follow-up.
- **Signature / quoted-history stripping.** v1 ingress is for external automated senders sending clean bodies; signature stripping lands with the reply-by-email half where humans actually quote prior messages.
- **Bouncing failed messages.** No bounce email is ever generated. The operator's only diagnostic is the structured log. This is a deliberate security posture (no enumeration via bounces, no reciprocal loops).
- **Dead-letter folder for poison messages.** A malformed message gets marked `\Seen` after one processing attempt and is never reprocessed, but the original copy stays in the polled mailbox. Tracked as `LC-77-DEAD-LETTER`.
- **Exactly-once dedup.** v1 is at-least-once with `\Seen`-after-attempt. A crash between processing and the `\Seen` STORE will reprocess the message on the next tick (rare duplicate). Tracked as `LC-77-MID-DEDUP`.
- **Voice-format attachments.** Email attachments cannot be voice messages in v1; voice has a `MediaRecorder` origin emails don't produce.

## Privacy notes

- **EXIF stripped on image upload.** Same pipeline web uploads use. GPS, camera-model, capture-time, and software-version metadata are all discarded on the decode-then-re-encode round-trip.
- **The sender's From address is NOT stored anywhere chat-visible.** The synthetic actor's display name is the inbox's configured `name`, never the sender's. If you need to know who sent a particular polled message, check the IMAP mailbox directly.
- **Polled mail stays in the IMAP mailbox.** lets-chat does not delete it; it just marks `\Seen`. Configure your provider's retention if you want it cleaned up.

## Security notes

- **The ingress address is a bearer secret.** Anyone who learns it can post to the room. Treat it like a webhook URL: don't paste it into shared docs without thinking, don't log it in your monitoring tool's outbound history.
- **To rotate**: create a new inbox in the per-room admin page, then revoke the old one. Mail to the old address starts dropping with `reason=revoked_inbox` on the next poll tick.
- **The IMAP password is stored AES-256-GCM-sealed in `settings.db`.** Same pattern as the VAPID keypair. A `settings.db` leak does not yield a usable IMAP password without `LETS_CHAT_SECRET_KEY` from the operator's environment.
- **SMTP password (used for outbound digest / verification mail) is currently stored plaintext.** This is a known inconsistency; tracked as `LC-77-SMTP-SEAL`. It does not affect email ingress (which only reads the sealed IMAP password), but is worth knowing if `settings.db` ever leaks.

## Related modules

- `crate::email_ingress::poll`: the IMAP poll loop + `process_polled_message` per-message handler.
- `crate::email_ingress::resolve`: header-precedence address-to-inbox resolution.
- `crate::email_ingress::parse`: MIME body + attachment-candidate extraction.
- `crate::email_ingress::attachments`: per-attachment upload pipeline.
- `crate::email_ingress::actor`: synthetic-actor post orchestration.
- `crate::db::email_inbox`: per-room inbox CRUD.
- `crate::db::imap_config`: AES-256-GCM-sealed IMAP creds.
- `crate::routes::email_inboxes`: per-room admin HTTP routes.
- `crate::routes::admin::post_imap_settings`: admin IMAP-settings HTTP route.
