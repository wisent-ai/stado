//! Two readings compared: what each process spent between them, and which
//! processes grow fast enough to use up the memory left ([`runaway`]).

pub mod runaway;

use std::time::Duration;

use serde::Serialize;

use super::{Counters, DiskBytes, Reading};
use runaway::{Horizon, Runaway};

/// One process's load between two readings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessLoad {
    pub pid: i32,
    pub program: String,
    /// The process was not in the earlier reading, so everything it spent
    /// in its lifetime was spent in this interval.
    pub started_in_interval: bool,
    pub cpu_seconds: f64,
    /// CPU of children it reaped in the interval; absent where the kernel
    /// keeps no such counter this host's reader takes.
    pub children_cpu_seconds: Option<f64>,
    pub resident_bytes: u64,
    /// How much the resident size changed since the earlier reading, negative
    /// when it shrank; all of it for a process that started in the interval.
    pub resident_change_bytes: i64,
    pub disk_read_bytes: Option<u64>,
    pub disk_written_bytes: Option<u64>,
    /// Why the disk counters are absent: the kernel's refusal.
    pub disk_unreadable: Option<String>,
    /// Own and reaped children's CPU together, the order of the report.
    #[serde(skip)]
    spent: Duration,
}

/// A process the earlier reading held and the later one did not: it ended,
/// or its pid now names another process, so what it spent in the interval
/// is not known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ended {
    pub pid: i32,
    pub program: String,
}

/// A process the kernel listed and refused to read, with its error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unreadable {
    pub pid: i32,
    pub error: String,
}

/// The per-process load of one host between two readings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessesReport {
    /// `compared` when the kept reading was compared with this one,
    /// `first_reading` when there was none to compare with or the kept one
    /// could not be (`not_compared` says why); the next one is measured from
    /// this one either way.
    pub state: &'static str,
    /// Why a kept reading was not compared with this one: its CPU counters
    /// were converted another way. Absent when it was compared, or when this
    /// host kept no reading before.
    pub not_compared: Option<String>,
    /// The measured time between the two readings.
    pub interval_seconds: Option<f64>,
    /// Every process that started in the interval or spent CPU or moved
    /// bytes in it, the one that spent the most CPU, its own and its reaped
    /// children's, first.
    pub processes: Vec<ProcessLoad>,
    /// How many processes the later reading read.
    pub read: usize,
    pub ended: Vec<Ended>,
    pub unreadable: Vec<Unreadable>,
    /// Which processes grow fast enough to leave the host no memory before
    /// the next reading; absent on a first reading, which has no growth.
    pub memory: Option<Runaway>,
}

fn unreadable(reading: &Reading) -> Vec<Unreadable> {
    reading
        .unreadable
        .iter()
        .map(|(pid, error)| Unreadable {
            pid: *pid,
            error: error.clone(),
        })
        .collect()
}

/// The report of a reading with nothing to measure from yet: `not_compared`
/// says why the reading kept before could not be, when one was kept.
pub fn first(now: &Reading, not_compared: Option<String>) -> ProcessesReport {
    ProcessesReport {
        state: "first_reading",
        not_compared,
        interval_seconds: None,
        processes: Vec::new(),
        read: now.processes.len(),
        ended: Vec::new(),
        unreadable: unreadable(now),
        memory: None,
    }
}

/// A lifetime counter's growth since `earlier`, or all of it for a process
/// that started in the interval.
fn grown(now: u64, earlier: Option<u64>) -> u64 {
    match earlier {
        Some(before) => now - before,
        None => now,
    }
}

/// What `now` spent since `earlier`; `earlier` is `None` for a process that
/// started in the interval.
fn load(pid: i32, now: &Counters, earlier: Option<&Counters>) -> ProcessLoad {
    let cpu = Duration::from_nanos(grown(now.cpu_ns, earlier.map(|before| before.cpu_ns)));
    let children = match (
        now.children_cpu_ns,
        earlier.map(|before| before.children_cpu_ns),
    ) {
        (Some(after), Some(Some(before))) => Some(Duration::from_nanos(after - before)),
        (Some(after), None) => Some(Duration::from_nanos(after)),
        _ => None,
    };
    let disk_before: Option<&Result<DiskBytes, String>> = earlier.map(|before| &before.disk);
    let (disk_read_bytes, disk_written_bytes, disk_unreadable) = match (&now.disk, disk_before) {
        (Ok(after), Some(Ok(before))) => (
            Some(after.read - before.read),
            Some(after.written - before.written),
            None,
        ),
        (Ok(after), None) => (Some(after.read), Some(after.written), None),
        (Err(error), _) | (Ok(_), Some(Err(error))) => (None, None, Some(error.clone())),
    };
    let spent = match children {
        Some(children) => cpu + children,
        None => cpu,
    };
    ProcessLoad {
        pid,
        program: now.program.clone(),
        started_in_interval: earlier.is_none(),
        cpu_seconds: cpu.as_secs_f64(),
        children_cpu_seconds: children.map(|children| children.as_secs_f64()),
        resident_bytes: now.resident_bytes,
        resident_change_bytes: resident_change(now.resident_bytes, earlier),
        disk_read_bytes,
        disk_written_bytes,
        disk_unreadable,
        spent,
    }
}

/// The resident size's change since `earlier`, all of it for a process that
/// started in the interval, saturated at the bounds of `i64`.
fn resident_change(now: u64, earlier: Option<&Counters>) -> i64 {
    let before = earlier.map_or(0, |before| before.resident_bytes);
    let change = i128::from(now) - i128::from(before);
    match i64::try_from(change) {
        Ok(change) => change,
        // Past what `i64` holds: the furthest it can say, in the change's direction.
        Err(_) if change > 0 => i64::MAX,
        Err(_) => i64::MIN,
    }
}

/// Did the process spend nothing and move nothing between the readings?
fn unchanged(before: &Counters, after: &Counters) -> bool {
    before.cpu_ns == after.cpu_ns
        && before.children_cpu_ns == after.children_cpu_ns
        && before.disk == after.disk
}

/// What every process spent between `earlier` and `now`, and which of them
/// grow fast enough to use up `available` memory within `horizon`.
pub fn compare(
    earlier: &Reading,
    now: &Reading,
    horizon: Horizon,
    available: Result<i64, String>,
) -> Result<ProcessesReport, String> {
    let interval = now.taken_ns.checked_sub(earlier.taken_ns).ok_or_else(|| {
        format!(
            "the kept process reading was taken after this one ({} ns against {} ns since the \
             epoch): the system clock went back, so no interval can be measured",
            earlier.taken_ns, now.taken_ns
        )
    })?;
    let mut ended = Vec::new();
    for (pid, before) in &earlier.processes {
        match now.processes.get(pid) {
            Some(after) if before.same_process(after) => {}
            _ => ended.push(Ended {
                pid: *pid,
                program: before.program.clone(),
            }),
        }
    }
    let mut processes = Vec::new();
    for (pid, after) in &now.processes {
        let before = earlier
            .processes
            .get(pid)
            .filter(|before| before.same_process(after));
        if before.is_some_and(|before| unchanged(before, after)) {
            continue;
        }
        processes.push(load(*pid, after, before));
    }
    processes.sort_by_key(|process| std::cmp::Reverse(process.spent));
    let interval = Duration::from_nanos(interval);
    let memory = runaway::judge(&processes, interval, horizon, available);
    Ok(ProcessesReport {
        state: "compared",
        not_compared: None,
        interval_seconds: Some(interval.as_secs_f64()),
        processes,
        read: now.processes.len(),
        ended,
        unreadable: unreadable(now),
        memory: Some(memory),
    })
}
