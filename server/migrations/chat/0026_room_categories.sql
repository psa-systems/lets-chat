-- LC-79 redesign: per-enclave room categories managed by enclave
-- admin/mod/owner. Replaces the per-user `sidebar_categories` /
-- `sidebar_category_rooms` tables that used to live in auth.db (see
-- the matching auth migration that drops those).
--
-- One row in `room_category_assignments` per assigned room (PK on
-- room_id) so a room belongs to at most one category. Unassigned
-- rooms fall into the "All rooms" bucket at render time.
CREATE TABLE IF NOT EXISTS room_categories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    enclave_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (enclave_id) REFERENCES enclaves(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_room_categories_enclave_position
    ON room_categories(enclave_id, position);

CREATE TABLE IF NOT EXISTS room_category_assignments (
    room_id INTEGER PRIMARY KEY,
    category_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    FOREIGN KEY (room_id) REFERENCES rooms(id) ON DELETE CASCADE,
    FOREIGN KEY (category_id) REFERENCES room_categories(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_room_category_assignments_category
    ON room_category_assignments(category_id, position);
