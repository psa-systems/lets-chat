CREATE TABLE IF NOT EXISTS sidebar_categories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    position INTEGER NOT NULL,
    collapsed INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_sidebar_categories_user_position
    ON sidebar_categories(user_id, position);

CREATE TABLE IF NOT EXISTS sidebar_category_rooms (
    user_id TEXT NOT NULL,
    room_id INTEGER NOT NULL,
    category_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (user_id, room_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (category_id) REFERENCES sidebar_categories(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_sidebar_category_rooms_category
    ON sidebar_category_rooms(category_id, position);
