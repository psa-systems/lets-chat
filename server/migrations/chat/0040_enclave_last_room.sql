-- LC-143: remember the room a user last had open in each enclave, so
-- clicking the enclave reopens it instead of an enclave home page. One row
-- per (user, enclave); upserted on room open.
--
-- No FK (same rationale as branding/reminders: keep test migration lists
-- decoupled). A stale row pointing at a deleted/inaccessible room is handled
-- at read time - the landing handler validates the room is still listable in
-- the enclave before redirecting, and falls back to the default room.
CREATE TABLE IF NOT EXISTS enclave_last_room (
    user_id    TEXT    NOT NULL,
    enclave_id INTEGER NOT NULL,
    room_id    INTEGER NOT NULL,
    updated_at TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, enclave_id)
);
