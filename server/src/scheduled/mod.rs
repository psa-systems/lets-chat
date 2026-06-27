//! LC-62: scheduled message delivery.
//!
//! Public surface is the dispatcher + the shared validation helper; the
//! persistence layer at `crate::db::scheduled` (PR-A) and the routes at
//! `crate::routes::scheduled` (Task 4) round out the feature.

pub mod dispatcher;
pub mod recurrence;
pub mod validation;

pub use dispatcher::{run_dispatch_tick, DispatchOutcome, DispatchStats};
pub use validation::{check_delivery_gates, DenyReason};
