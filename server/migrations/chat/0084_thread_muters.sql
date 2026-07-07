-- LC-546: per-thread mute. The inverse of LC-310 thread following: a muter is
-- suppressed from the thread's reply notifications (WS toast + push + email)
-- even while still an auto-followed participant, so a member can turn down one
-- noisy thread without unfollowing every thread they have replied in. Mute
-- wins over follow at fan-out time (see routes::room::notify_thread_followers).
-- Standalone table keyed by the thread root message, mirroring thread_followers.
CREATE TABLE thread_muters (
    user_id     TEXT NOT NULL,
    parent_id   INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    room_id     INTEGER NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, parent_id)
);

CREATE INDEX idx_thread_muters_parent ON thread_muters (parent_id);
