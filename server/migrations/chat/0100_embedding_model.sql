-- LC-911: detect an embedding-model (or dimensionality) swap instead of
-- silently scoring stale vectors against a new model's vectors forever.
-- `model` is the second cache key alongside `dim`: `doc_chunks`'
-- source_content_hash lookup and `message_embeddings`' backfill selector both
-- compare it against the currently configured model name, so a swap makes the
-- existing refresh mechanisms treat every row as work to do again. Backfilled
-- to '' for existing rows, which never matches a real configured model name
-- and so is always treated as stale.
ALTER TABLE doc_chunks ADD COLUMN model TEXT NOT NULL DEFAULT '';
ALTER TABLE message_embeddings ADD COLUMN model TEXT NOT NULL DEFAULT '';
