-- LC-526: kudos / recognition. A kudos is given via the `/kudos @user <reason>`
-- slash command; each one records the giver, receiver, the room + its enclave
-- (for scoping the leaderboard), an optional reason, and the id of the
-- recognition message posted to the room. The per-enclave leaderboard is a
-- plain aggregate over this table. Kudos are additive only (no downvotes).

CREATE TABLE IF NOT EXISTS kudos (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    giver_id    TEXT    NOT NULL,
    receiver_id TEXT    NOT NULL,
    room_id     INTEGER NOT NULL,
    -- The room's enclave at give time, or NULL for a non-enclave room (e.g. a
    -- DM). NULL rows never appear on any enclave leaderboard.
    enclave_id  INTEGER,
    reason      TEXT,
    -- The posted recognition message; NULL if the post failed after recording.
    message_id  INTEGER,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_kudos_receiver ON kudos(receiver_id);
CREATE INDEX IF NOT EXISTS idx_kudos_giver ON kudos(giver_id);
CREATE INDEX IF NOT EXISTS idx_kudos_enclave ON kudos(enclave_id);
