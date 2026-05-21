//! Per-room message retention.
//!
//! Admin sets `rooms.retention_days = N`; a background sweep hard-deletes
//! messages in that room older than N days, cascading through every
//! referencing row via the FK actions verified in
//! `tests/retention_cascade.rs`. The actual destructive path is gated by
//! the `LETS_CHAT_RETENTION_SWEEP_ENABLED` env var (default off) because
//! the strict-vs-loose semantics question for thread retention is still
//! pending with the ticket author; this commit ships the loose-correct
//! shape (sweep-by-newest-reply) so that flipping to strict later means
//! dropping a single `NOT EXISTS` clause from the candidate predicate.
//!
//! No spawn wiring, no broadcast, no audit yet: those land in the next
//! commit. The sweep is callable from anywhere that has a chat pool, but
//! `run_retention_sweep` will no-op unless the env flag is set.

pub mod predicate;
pub mod sweep;
