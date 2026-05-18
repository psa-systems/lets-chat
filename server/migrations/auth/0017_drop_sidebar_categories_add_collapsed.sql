-- LC-79 redesign: categories are now per-enclave (managed by enclave
-- admin/mod/owner) rather than per-user. The old per-user tables in
-- auth.db are dropped. A new `collapsed_categories` table replaces the
-- per-user-collapsed flag that used to live on `sidebar_categories`:
-- each user can independently collapse / expand any category in any
-- enclave they belong to. Category metadata + assignments now live in
-- chat.db (see migration 0026 there).
DROP TABLE IF EXISTS sidebar_category_rooms;
DROP TABLE IF EXISTS sidebar_categories;

CREATE TABLE IF NOT EXISTS collapsed_categories (
    user_id TEXT NOT NULL,
    category_id INTEGER NOT NULL,
    PRIMARY KEY (user_id, category_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
