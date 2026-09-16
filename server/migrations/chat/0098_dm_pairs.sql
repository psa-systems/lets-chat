-- LC-909: let the database arbitrate the assistant-bot (and any other) DM
-- find-or-create. `find_dm_room` / `create_dm_room` had no constraint tying a
-- user pair to at most one `dm` room, so two concurrent renders of the
-- support bubble (an HTTP request and a WS-driven re-render, or two page
-- loads) could each see no existing room and each create one. This table
-- gives the pair a unique index: `create_dm_room` inserts into it inside the
-- same transaction as the room/room_members rows, canonically ordering the
-- two user ids (plain lexicographic comparison) so the pair is
-- order-independent, and a losing concurrent insert fails the UNIQUE
-- constraint instead of silently creating a duplicate room.
CREATE TABLE IF NOT EXISTS dm_pairs (
    room_id  INTEGER PRIMARY KEY REFERENCES rooms(id) ON DELETE CASCADE,
    user_lo  TEXT NOT NULL,
    user_hi  TEXT NOT NULL,
    UNIQUE (user_lo, user_hi)
);

-- Backfill from existing DM rooms. `INSERT OR IGNORE` plus `ORDER BY r.id`
-- means that where a pair already has more than one `dm` room (the bug this
-- migration closes), the lowest-id room wins the pair and becomes the one
-- `find_dm_room`'s new `ORDER BY r.id LIMIT 1` would have returned anyway;
-- the newer duplicate rooms are left as-is but no longer claim the pair.
INSERT OR IGNORE INTO dm_pairs (room_id, user_lo, user_hi)
SELECT r.id,
       MIN(m.user_id),
       MAX(m.user_id)
  FROM rooms r
  JOIN room_members m ON m.room_id = r.id
 WHERE r.room_type = 'dm'
 GROUP BY r.id
HAVING COUNT(*) = 2
 ORDER BY r.id;
