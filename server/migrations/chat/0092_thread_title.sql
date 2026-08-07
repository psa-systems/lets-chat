-- LC-668: an AI-generated short title for a thread, stored on its root message.
-- NULL until the thread reaches the reply threshold and a title is generated;
-- only ever set on thread roots (parent_id IS NULL). Shown in the thread panel
-- header. Read via a dedicated getter, like assistant_enabled / digest_last_at.
ALTER TABLE messages ADD COLUMN thread_title TEXT;
