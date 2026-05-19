-- LC-93: per-user and per-enclave upload quotas.
--
-- `enclaves.quota_bytes IS NULL` means unlimited (matches today's
-- behavior); a non-null value caps the SUM of size_bytes of every
-- file_upload attached to a message in any room of the enclave.
ALTER TABLE enclaves ADD COLUMN quota_bytes INTEGER;

-- Per-user quota lives in its own table rather than on the auth.db
-- users row so the upload-time check stays single-domain: the upload
-- handler already talks to chat.db for file_uploads / messages, and
-- the quota lookup joins cleanly against those.
--
-- Absence of a row means the user is unlimited. The `updated_at`
-- column is for the admin audit-log / future quota-change feed.
CREATE TABLE IF NOT EXISTS user_storage_quotas (
    user_id     TEXT PRIMARY KEY,
    quota_bytes INTEGER NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
