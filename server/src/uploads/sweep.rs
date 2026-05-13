//! Orphan-upload garbage collection. Dedup-aware per-row sweep that deletes
//! `file_uploads` rows whose `message_id IS NULL` and whose `created_at` is
//! older than the configured threshold. Filled in by Task 5.
