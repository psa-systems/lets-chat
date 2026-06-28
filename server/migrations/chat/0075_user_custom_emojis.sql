-- LC-482: user-level (personal) custom emoji alongside enclave emoji.
--
-- The custom_emojis table was enclave-only (enclave_id NOT NULL). A personal
-- emoji has no enclave, so we make enclave_id nullable and add a user_id scope:
-- exactly one of (enclave_id, user_id) is set per row (CHECK), keeping a single
-- id space so /api/emojis/{id} and the `:shortcode:` -> <img> render path are
-- unchanged. SQLite cannot drop a NOT NULL constraint in place, so rebuild the
-- table (a new file, never editing the shipped 0017) and copy existing rows in
-- as enclave-scoped (user_id NULL).
CREATE TABLE custom_emojis_new (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    enclave_id   INTEGER REFERENCES enclaves(id) ON DELETE CASCADE,
    user_id      TEXT,
    shortcode    TEXT    NOT NULL,
    storage_path TEXT    NOT NULL,
    mime_type    TEXT    NOT NULL,
    size_bytes   INTEGER NOT NULL,
    uploaded_by  TEXT    NOT NULL,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    -- Exactly one scope: enclave XOR user.
    CHECK ((enclave_id IS NOT NULL) <> (user_id IS NOT NULL))
);

INSERT INTO custom_emojis_new
    (id, enclave_id, user_id, shortcode, storage_path, mime_type, size_bytes, uploaded_by, created_at)
SELECT id, enclave_id, NULL, shortcode, storage_path, mime_type, size_bytes, uploaded_by, created_at
FROM custom_emojis;

DROP TABLE custom_emojis;
ALTER TABLE custom_emojis_new RENAME TO custom_emojis;

CREATE INDEX idx_custom_emojis_enclave ON custom_emojis(enclave_id);
CREATE INDEX idx_custom_emojis_user ON custom_emojis(user_id);
-- Shortcode uniqueness is per-scope (partial unique indexes).
CREATE UNIQUE INDEX ux_custom_emojis_enclave_code
    ON custom_emojis(enclave_id, shortcode) WHERE enclave_id IS NOT NULL;
CREATE UNIQUE INDEX ux_custom_emojis_user_code
    ON custom_emojis(user_id, shortcode) WHERE user_id IS NOT NULL;
