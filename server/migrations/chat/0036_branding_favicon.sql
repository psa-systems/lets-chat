-- LC-142: per-scope favicon (LC-96 follow-up). Mirrors `logo_upload_id`:
-- nullable, no FOREIGN KEY (same rationale as 0034 - keeping the column
-- FK-free avoids forcing every hand-rolled test migration list to pull in
-- 0012_uploads.sql; the orphan sweep exempts whatever this points at).
--
-- v1 is global-only; per-enclave favicons are a stretch goal (browsers
-- cache favicons aggressively, so the per-enclave payoff is marginal). The
-- column lives on every scope row for symmetry, but only the global row is
-- written today.
ALTER TABLE branding ADD COLUMN favicon_upload_id INTEGER;
