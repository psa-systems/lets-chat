-- LC-77-MID-DEDUP (#202): exactly-once dedup table for the IMAP poll
-- loop. Keyed on the HMAC-SHA256 of the message's RFC 5322 Message-ID
-- header under LETS_CHAT_SECRET_KEY so the table itself does not leak
-- which correspondents the operator has been receiving mail from.
--
-- Scoping decision: the dedup key is GLOBAL across the polled mailbox,
-- not per-inbox. The original ticket proposed a `(inbox_id,
-- message_id_hash)` composite key. The polled mailbox now serves both
-- the per-room inbox path (`email_ingress::actor::post_email_message`)
-- AND the reply-by-email path (`email_ingress::reply_actor::post_reply_message`,
-- LC-77-REPLY stage 2), and the reply path has no `inbox_id`. A single
-- global key covers both paths without an awkward NULL/sentinel
-- column.
--
-- The `processed_at` index supports the 30-day TTL sweep that
-- piggybacks on the hourly orphan sweeper. Rows older than 30 days are
-- dropped opportunistically; a Message-ID re-issued by a sender more
-- than 30 days after the original is treated as a fresh message.

CREATE TABLE IF NOT EXISTS processed_message_ids (
    message_id_hash TEXT PRIMARY KEY,
    processed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_processed_message_ids_at
    ON processed_message_ids (processed_at);
