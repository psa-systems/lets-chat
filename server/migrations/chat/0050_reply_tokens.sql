-- LC-77-REPLY (#201) stage 1: side table for reply-by-email tokens.
--
-- A per-mention notification email carries `Reply-To: reply-<token>@<ingress-
-- domain>`. Replying to the email lands at the polled mailbox; the email-
-- ingress resolver in stage 2 extracts the token from the local part, looks
-- up this row, verifies expires_at > now(), and posts the reply as the user
-- the row points at.
--
-- `token` is a 32-byte random base32-encoded string (~52 chars). It is the
-- credential; storing the plaintext is fine because (a) it only leaks to a
-- single recipient's verified email address, (b) it's bounded by
-- expires_at (7 days), (c) it binds to one (user_id, message_id) pair so
-- it can't be replayed against other messages or other users.
--
-- ON DELETE CASCADE on message_id: deleting a message reaps its outstanding
-- reply tokens. A reply to a deleted message resolves to no row and drops
-- the same way an unknown secret address drops in the v1 ingress path.
CREATE TABLE IF NOT EXISTS reply_tokens (
    token       TEXT    PRIMARY KEY,
    user_id     TEXT    NOT NULL,
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    issued_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    expires_at  TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_reply_tokens_expires ON reply_tokens (expires_at);
