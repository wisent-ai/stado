//! Job scheduler: pick queued jobs and create instances.
//!
//! Port of `stado/scheduler/scheduler.py`. Routing rules:
//! - job.pin_to_provider=True + job.provider="local" -> only local agent claims
//! - job.pin_to_provider=True + job.provider=<X>     -> only provider X claims
//! - job.pin_to_provider=False (default)             -> any consumer with
//!   capacity can claim. The Cloud Function (this file) skips a job ONLY if
//!   its capacity cannot satisfy the job (no quota, or cost cap exceeds
//!   available SKU rate); the local agent then has a chance.
//!
//! A job whose create_instance call failed is offered again on the next
//! tick. It is not skipped for a window: the providers' own exhaustion
//! marks keep a retry from repeating the zones that just refused (every
//! unmarked zone is tried, then the one marked longest ago), so a wedged job
//! costs one bounded round of provider calls per tick, and the local agent
//! claims it in the meantime whenever it fits.
//!
//! Deviation: Python defines a `_attempt` closure (the legacy 1-VM-per-job
//! dispatch path) that is never called — agent-mode dispatch
//! ([`dispatch::agent::dispatch_agent_vms`]) fully replaced it. The dead
//! closure is not ported; its per-job behaviors (empty-machine_type
//! failure guard, backoff accounting, no-preemptible policy) live on in
//! the bucketed agent dispatch.

mod error;
mod passes;
mod support;

pub use error::SchedulerError;
pub use passes::local_pack::LOCAL_ADMISSION_BUFFER_GB;
pub use passes::run::{schedule_queued_jobs, schedule_queued_jobs_routed};
pub use support::rates::accel_hourly_rate;
pub(crate) use support::reporting::log;
