//! LC-77-REPLY (#201): outbound notification email surfaces beyond the
//! hourly digest. v1 ships per-message mention + DM notifications
//! ([`notification::dispatch_mention_notification`]); reply ingress lands
//! in stage 2.
//!
//! The existing single-shot mail sites (password reset, email verification,
//! login alert) intentionally stay where they are under `routes/`; they
//! are tightly coupled to their HTTP handler and have no business under a
//! shared `email/` module. The digest similarly stays at `crate::digest`
//! because it owns its own background-tick state machine. This module is
//! the home for new outbound surfaces that fire from inside chat
//! mutations (mention reconcile, DM send, ...) rather than from an HTTP
//! handler or a background tick.

pub mod notification;
