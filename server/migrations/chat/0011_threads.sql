-- Threads: replies hang off a parent message via parent_id.
-- Top-level messages have parent_id IS NULL. Replies are one level only;
-- the application enforces that parent_id, when set, points at a message
-- whose own parent_id IS NULL.
ALTER TABLE messages ADD COLUMN parent_id INTEGER REFERENCES messages(id) ON DELETE CASCADE;
CREATE INDEX IF NOT EXISTS idx_messages_parent ON messages(parent_id);
