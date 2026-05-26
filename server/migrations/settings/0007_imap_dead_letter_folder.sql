-- LC-77-DEAD-LETTER (#203): optional dead-letter IMAP folder for the
-- poll loop. When set, every message that `process_polled_message`
-- drops (parse_fail, address_no_match, revoked_inbox, loop_detected,
-- rate_limited, reply_expired, duplicate, internal_error) is COPYd
-- into this folder before the source UID is marked `\Seen`, so an
-- operator can inspect rejected mail without grepping logs.
--
-- NULL = feature off; the poll loop falls back to v1 always-`\Seen`
-- behavior with the structured drop log as the only diagnostic.
--
-- The operator must create the folder at their IMAP provider; lets-chat
-- does not auto-create it. A COPY to a non-existent folder fails IMAP-
-- side with NO; the poll loop logs INFO and proceeds with the `\Seen`
-- STORE so a misconfigured dead-letter folder cannot block the queue.

ALTER TABLE imap_inbox_config ADD COLUMN dead_letter_folder TEXT;
