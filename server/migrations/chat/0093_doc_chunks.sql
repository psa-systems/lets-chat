-- LC-712: AI help desk, Phase 1. Documentation knowledge base for the support
-- assistant (`/support`). Chunks of the product docs (Mokosh, Bunyip, Let's
-- Chat, ...) are fetched from their published docs sites, split on headings,
-- embedded, and stored here for cross-product semantic retrieval.
--
-- This is deliberately a NEW table, not a reuse of `message_embeddings`: that
-- sidecar is keyed 1:1 to `messages(id)` with a mandatory `room_id` and is
-- searched per-room, so it cannot hold room-less external documents. The vector
-- storage format is shared though: `vec` is a little-endian f32 BLOB
-- (`embeddings::vec_to_bytes`) and `dim` its length, so a model swap that
-- changes dimensionality is detectable, exactly as in `message_embeddings`.
--
-- `content_hash` is the SHA-256 of the source page's extracted text; every chunk
-- of a page carries the same hash so a refresh can skip a page whose content is
-- unchanged. `(source_url, chunk_index)` is unique so a re-index upserts in
-- place; a page's chunks are deleted before re-insert so a page that shrank does
-- not leave stale trailing chunks.
CREATE TABLE doc_chunks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    product      TEXT NOT NULL,
    source_url   TEXT NOT NULL,
    title        TEXT NOT NULL,
    heading      TEXT NOT NULL DEFAULT '',
    chunk_index  INTEGER NOT NULL,
    body         TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    dim          INTEGER NOT NULL,
    vec          BLOB NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (source_url, chunk_index)
);

-- Retrieval scans the whole corpus (or one product) and cosine-ranks in Rust;
-- the product index keeps the optional product-scoped scan cheap.
CREATE INDEX idx_doc_chunks_product ON doc_chunks (product);
