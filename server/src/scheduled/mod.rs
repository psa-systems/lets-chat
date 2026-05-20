//! LC-62: scheduled message delivery.
//!
//! Public surface is the dispatcher; the persistence layer at
//! `crate::db::scheduled` (PR-A) and the routes at `crate::routes::scheduled`
//! (Task 4) round out the feature.

pub mod dispatcher;

pub use dispatcher::{run_dispatch_tick, DispatchOutcome, DispatchStats};
