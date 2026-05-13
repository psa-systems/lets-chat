-- Quote-reply: a top-level message can reference an earlier message in the
-- same room as the context it is replying to. Distinct from threads
-- (parent_id), which collapse the reply into a side panel. A quote-reply
-- still appears in the main timeline; the referenced message renders as an
-- inline chip above the body.
--
-- ON DELETE SET NULL: when the original quoted message is hard-deleted the
-- chip silently drops out rather than dragging the quoting message with it.
-- Soft-delete (deleted_at) is handled at render time by suppressing the chip.
ALTER TABLE messages ADD COLUMN quote_id INTEGER REFERENCES messages(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_messages_quote ON messages(quote_id) WHERE quote_id IS NOT NULL;
