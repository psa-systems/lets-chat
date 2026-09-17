-- LC-549: semantic / related-message search.
--
-- Sidecar table (NOT a column on `messages`) so the hot message SELECT surface
-- and RawMessage mapping stay untouched: adding a column there is a known drift
-- trap (see LC-547). One embedding per message, populated best-effort in the
-- background when an operator embeddings endpoint is configured.
--
-- `vec` is a little-endian f32 BLOB (see `embeddings::vec_to_bytes`); `dim` is
-- the vector length so a model swap that changes dimensionality is detectable.
-- ON DELETE CASCADE ties the embedding to its message: a hard-deleted or
-- self-destructed (LC-547) message drops its embedding automatically.
CREATE TABLE message_embeddings (
    message_id INTEGER PRIMARY KEY REFERENCES messages (id) ON DELETE CASCADE,
    room_id    INTEGER NOT NULL,
    dim        INTEGER NOT NULL,
    vec        BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Nearest-neighbour ranking scans one room's embeddings at a time.
CREATE INDEX idx_message_embeddings_room ON message_embeddings (room_id);
