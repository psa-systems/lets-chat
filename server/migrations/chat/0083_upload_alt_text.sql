-- LC-537: per-image alt text for accessibility. NULL means "no author-provided
-- alt"; the renderer then falls back to the filename (v1 behaviour). Set by the
-- uploader after posting via POST /api/files/{id}/alt.
ALTER TABLE file_uploads ADD COLUMN alt_text TEXT;
