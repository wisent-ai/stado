//! The passes a tick runs, in the order the Python `schedule_queued_jobs`
//! ran them: the metadata-only prefilter that orders the candidate window,
//! the cost-optimal local pack that yields jobs to live local agents, and
//! the driver that reads capacity and hands the survivors to agent-VM
//! dispatch.

pub(super) mod local_pack;
mod prefilter;
pub(super) mod run;
