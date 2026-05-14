-- System messages: server-authored notices (e.g. "started a call") that
-- render as a centered, non-interactive line rather than a normal chat
-- bubble. `user_id` still records who triggered the event.
ALTER TABLE messages ADD COLUMN is_system INTEGER NOT NULL DEFAULT 0;
