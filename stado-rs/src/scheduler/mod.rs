//! Scheduling and quota subsystems.
//!
//! Port of `stado/scheduler/`. `scheduler`: the per-tick dispatch engine
//! (`schedule_queued_jobs`: metadata prefilter, dynamic cap, cost-optimal
//! local pack, agent-VM dispatch); `quota`: live cloud quota + reservation
//! overlay + running counts (READ side only); `dispatch::agent`: agent-VM
//! bucketing/dispatch with stockout escalation; `dispatch::box`: the Box
//! lease state machine (reconcile + dispatch + runtime + bounded output);
//! `dispatch::quota_request` / `quota_replies` / `quota_skus`: the quota
//! WRITE path (Cloud Quotas / ARM quota puts, az support-ticket replies,
//! catalog + status enumeration); `makespan` (+ `makespan::history`):
//! LPT/greedy makespan-minimizing job->agent assignment; `cost`: cost
//! report/estimate from finished-job wall-times; `skip_done`: the
//! (currently disabled in Python) HF pre-dispatch filter.

pub mod cost;
pub mod dispatch;
pub mod makespan;
pub mod quota;
// The Python module is stado/scheduler/scheduler.py — same nested name.
#[allow(clippy::module_inception)]
pub mod scheduler;
pub mod skip_done;
