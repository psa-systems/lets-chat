-- LC-310: thread following. A follower is notified (WS toast + push + email,
-- honoring per-room mute) of every new reply to a thread, even without an
-- @mention. Replying auto-follows the replier; the thread root's author is
-- auto-followed too. Follows are NOT mentions rows (a follow is not a
-- per-message mention, and the edit path's reconcile would delete them), so
-- this is a standalone subscription table keyed by the thread root message.
CREATE TABLE thread_followers (
    user_id     TEXT NOT NULL,
    parent_id   INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    room_id     INTEGER NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, parent_id)
);

CREATE INDEX idx_thread_followers_parent ON thread_followers (parent_id);
