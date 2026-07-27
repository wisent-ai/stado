//! Monitor subsystem — Rust port of `stado/monitor/`.
//!
//! - [`monitor`]: `check_running_jobs` (finalize COMPLETED/FAILED, requeue on
//!   vanished instance / stale heartbeat / Spot preemption) and
//!   `reap_dead_agents` (dead / never-worked / wedged VM reaper).
//! - [`heartbeat_guard`]: per-job heartbeat + GCS checkpoint freshness
//!   signals that defer reaps of productive VMs.
//! - [`alerts`]: multi-channel alert fan-out (Slack, Telegram, SendGrid,
//!   GCP Pub/Sub over REST).
//! - [`billing`]: `billing_health/credits.json` collector (BigQuery billing
//!   export + Azure ARM balance).
//! - [`host_health`]: read side of the `host_health/<host>.json` beacon.
//! - [`reap`]: by-run reaper deleting per-job blobs of fully-terminal runs.
//!
//! The Cloud Function entry point (`stado/cloud_function/main.py`,
//! `monitor_jobs`) composes the same tick as `crate::coordinator`: fire due
//! schedules -> normalize queue sizing -> makespan assign -> per provider
//! (check running + reap dead agents + schedule queued) -> run reaper ->
//! billing collect. It needs no separate port; `coordinator.rs` is the
//! single tick implementation and its module docs note the parity.

pub mod alerts;
pub mod billing;
pub mod heartbeat_guard;
pub mod host_health;
// The Python module is stado/monitor/monitor.py — same nested name.
#[allow(clippy::module_inception)]
pub mod monitor;
pub mod reap;
