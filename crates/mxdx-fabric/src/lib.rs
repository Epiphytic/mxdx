#![deprecated(
    note = "Use mxdx-worker and mxdx-coordinator instead. This crate will be removed in v2.0."
)]

pub mod capability_index;
pub mod claim;
pub mod coordinator;
pub mod failure;
pub mod process_worker;
pub mod sender;
pub mod worker;

pub use mxdx_types::events::fabric::*;
pub use worker::{EVENT_CAPABILITY, EVENT_CLAIM, EVENT_HEARTBEAT, EVENT_RESULT, EVENT_TASK};
