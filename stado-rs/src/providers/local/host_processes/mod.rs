//! What each process on this host consumed between two readings, read from
//! the kernel inside Stado's own process.
//!
//! A host under load is diagnosed by naming the processes that spent its CPU
//! and disk. `top` and `ps` are other programs: an agent session, a sandboxed
//! shell or a host-exec allowlist can be refused them, and `ps` reports CPU
//! time in whole seconds, too coarse for a sample of a few seconds. So the
//! kernel's own per-process counters are read, the reading is kept on the
//! host, and every reading is reported against the one before it: CPU
//! seconds, CPU seconds of children reaped in between (where the kernel
//! keeps them), resident memory, and bytes read from and written to storage.
//!
//! Nothing here waits. The interval is the time between two readings of one
//! reader, each taken because something asked: the host's beacon pass keeps
//! its readings, an operator's `stado host processes` (and Stado Desktop's
//! Processes section, which runs it) keeps its own (`store::Keeper`), so
//! neither shortens the other's interval. The report states that interval
//! as measured.
//!
//! The reaped-children figure is what names a process that runs many short
//! commands: a program that lives a fraction of a second is in neither
//! reading, but its CPU is added to its parent's children counter when the
//! parent reaps it.
//!
//! Each process's resident growth between the readings is judged against the
//! memory the kernel says is still available ([`report::runaway`]), read in
//! this process like the counters: the report names the processes that, at
//! the rate they grew, leave the host none before the next reading is taken.

#[cfg(target_os = "macos")]
mod darwin;
#[cfg(target_os = "linux")]
mod linux;
pub mod report;
mod store;

#[cfg(target_os = "macos")]
use darwin as platform;
#[cfg(target_os = "linux")]
use linux as platform;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub use report::runaway::Horizon;
pub use report::{ProcessLoad, ProcessesReport};

/// What one process had consumed in its lifetime when it was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    /// The executable's path, or the kernel's name for the process.
    pub program: String,
    /// The kernel's record of when the process started, in its own units.
    /// Equal records under one pid are one process; a different one is the
    /// pid reused. `None` where this reader takes no start record from the
    /// kernel; the program and a CPU total that never decreases then stand
    /// for the identity.
    pub started: Option<u64>,
    pub cpu_ns: u64,
    /// CPU of the children this process has reaped; `None` where the kernel
    /// keeps no such counter this reader takes.
    pub children_cpu_ns: Option<u64>,
    pub resident_bytes: u64,
    /// Bytes read from and written to storage, or the kernel's refusal of
    /// that read for this process.
    pub disk: Result<DiskBytes, String>,
}

/// Bytes one process moved to and from storage in its lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskBytes {
    pub read: u64,
    pub written: u64,
}

/// Every process the kernel listed, read once, and when.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Reading {
    /// Wall-clock time of the reading, nanoseconds since the Unix epoch, so
    /// two readings taken by different processes measure their interval.
    pub taken_ns: u64,
    pub processes: BTreeMap<i32, Counters>,
    /// Listed processes whose counters the kernel refused, with its error.
    pub unreadable: BTreeMap<i32, String>,
    /// How the reader turned each process's CPU counters into nanoseconds
    /// (`mach_timebase_info`, `schedstat`). Two readings converted different
    /// ways do not measure one interval; a reading kept by a build that did
    /// not say (`None`) converted on macOS by a ratio measured afresh each
    /// time, which made every long-lived process look new.
    #[serde(default)]
    pub conversion: Option<String>,
}

impl Counters {
    /// Is `later` a reading of the same process this one read? A process's
    /// lifetime counters never decrease, so a reading under the same pid
    /// whose counters went down is another process that took the pid.
    fn same_process(&self, later: &Counters) -> bool {
        let started = match (self.started, later.started) {
            (Some(before), Some(after)) => before == after,
            _ => self.program == later.program,
        };
        let children = match (self.children_cpu_ns, later.children_cpu_ns) {
            (Some(before), Some(after)) => after >= before,
            (None, None) => true,
            _ => false,
        };
        let disk = match (&self.disk, &later.disk) {
            (Ok(before), Ok(after)) => after.read >= before.read && after.written >= before.written,
            _ => true,
        };
        started && later.cpu_ns >= self.cpu_ns && children && disk
    }
}

/// Read every process now.
pub fn read() -> Result<Reading, String> {
    let taken = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("the system clock reads before the Unix epoch: {error}"))?;
    let taken_ns = u64::try_from(taken.as_nanos()).map_err(|error| {
        format!("the system clock is past what nanoseconds since the epoch hold: {error}")
    })?;
    let mut reading = platform::read()?;
    reading.taken_ns = taken_ns;
    Ok(reading)
}

/// The beacon's reading, every process reported against the reading before
/// and kept in its place. The repeating health role (`cadence` its period)
/// keeps the beacon's own reading and judges memory growth against its
/// period, when its next reading comes. A one-off `stado host beacon collect`
/// (no cadence) is measured as an operator's run is, from the operator's
/// reading, so a collection by hand never shortens the beacon's interval.
pub fn read_and_compare(cadence: Option<Duration>) -> Result<ProcessesReport, String> {
    match cadence {
        Some(period) => compare_kept(store::Keeper::Beacon, Horizon::BeaconPeriod(period)),
        None => compare_kept(store::Keeper::Operator, Horizon::Measured),
    }
}

/// An operator's reading (`stado host processes`): reported against the
/// operator's reading before, never the beacon's, with the next reading
/// `horizon` away.
pub fn read_and_compare_within(horizon: Horizon) -> Result<ProcessesReport, String> {
    compare_kept(store::Keeper::Operator, horizon)
}

/// The first reading a keeper takes has nothing to be compared with; its
/// report says so and lists no load, and the next one is measured from it.
/// A kept reading whose CPU counters were converted another way, or by a
/// build that did not say how, is no earlier reading of the same counters:
/// this one is reported as a first reading naming why, and the next is
/// measured from it, so no process is taken for new and judged to have
/// grown by all it holds.
fn compare_kept(keeper: store::Keeper, horizon: Horizon) -> Result<ProcessesReport, String> {
    let now = read()?;
    let previous = store::previous(keeper)?;
    store::keep(keeper, &now)?;
    match previous {
        Some(previous) if previous.conversion == now.conversion => {
            // Only what a new allocation can obtain is judged against, read
            // as the capacity publication reads it, by this process: no
            // other program runs for it.
            let available =
                crate::providers::local::host_memory::reading::read_available_and_total()
                    .map(|(available, _)| available);
            report::compare(&previous, &now, horizon, available)
        }
        Some(previous) => Ok(report::first(&now, Some(not_compared(&previous, &now)))),
        None => Ok(report::first(&now, None)),
    }
}

/// Why `kept` is not compared with `now`: how each converted its CPU
/// counters to nanoseconds.
fn not_compared(kept: &Reading, now: &Reading) -> String {
    let said = |reading: &Reading| match &reading.conversion {
        Some(conversion) => format!("by {conversion}"),
        None => "by a build that did not record how".to_string(),
    };
    format!(
        "the reading kept before converted CPU counters {}, this one {}, so it measures no \
         interval of the same counters; the next run is measured from this one",
        said(kept),
        said(now)
    )
}
