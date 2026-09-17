-- LC-925: cache the fetched, sniffed, size-capped thumbnail bytes alongside
-- the link preview row they belong to, keyed by the same url_hash. Before
-- this, get_unfurl_image re-fetched the remote origin on every request for a
-- url_hash, even seconds apart from a different viewer, because
-- link_previews stored only the source URL. image_fetched_at is tracked
-- separately from the row's own fetched_at: the page metadata and the image
-- bytes can be refreshed at different times (an og:image fetch can fail while
-- the page metadata still updates).
ALTER TABLE link_previews ADD COLUMN image_data BLOB;
ALTER TABLE link_previews ADD COLUMN image_content_type TEXT;
ALTER TABLE link_previews ADD COLUMN image_fetched_at TEXT;
