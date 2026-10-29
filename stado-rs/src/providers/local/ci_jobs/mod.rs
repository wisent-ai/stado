//! How many CI jobs this host's own runners are executing now, counted from
//! its process table.
//!
//! A GitHub Actions runner listens with `Runner.Listener` and, once it has
//! acquired a job, starts a `Runner.Worker` process that executes it
//! (<https://depot.dev/blog/github-actions-runner-architecture-part-1-the-listener>,
//! "Once the job is acquired, the listener starts the Worker process").
//! Counting those processes is counting the jobs in flight, whichever runner
//! installation or user started them. The count is published with the
//! host's capacity, so placed work is kept off a host while it builds for CI.

#[cfg(target_os = "macos")]
mod darwin;
#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
use darwin as platform;
#[cfg(target_os = "linux")]
use linux as platform;

/// The executable name of the process a runner starts for each job.
const JOB_PROCESS: &str = "Runner.Worker";

/// The CI jobs running on this host, or why the process table could not be
/// read.
pub fn running() -> Result<u64, String> {
    platform::process_names().map(|names| {
        names
            .iter()
            .filter(|name| name.as_str() == JOB_PROCESS)
            .count() as u64
    })
}
