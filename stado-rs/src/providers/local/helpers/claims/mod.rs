//! What this agent is allowed to claim.
//!
//! [`eligibility`] holds the per-job rules and the name of the first one that
//! refuses; [`queue_scan`] applies them across the whole queue to answer
//! whether anything here is claimable at all.

pub mod eligibility;
pub mod queue_scan;
