//! Services and units the fleet runs: the startup template a dispatched VM
//! executes, the queue switch that governs dispatch, and the channels that
//! page an operator when either goes wrong.

pub(in crate::doctor) mod alerts;
pub(in crate::doctor) mod queue;
pub(in crate::doctor) mod template;
