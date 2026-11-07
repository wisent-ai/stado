//! Which processes grow fast enough to run this host out of memory before the
//! next reading is taken.
//!
//! A machine that runs out of application memory pauses every application at
//! once, and the process behind it is rarely the one in front of the
//! operator: a desktop app whose pipe reader kept running at end of file, or
//! one agent session that started dozens of others. Two readings already hold
//! each process's resident size, so each one's growth over a measured interval
//! is known. At that rate, against the memory the kernel says a new allocation
//! can still obtain, the growth answers how long the host has left.
//!
//! The answer names the fewest processes, largest growth first, whose growth
//! together exhausts the available memory within the horizon: one process
//! when it does so alone, several when many each grow a little. The horizon
//! is when the next reading comes: the seconds an operator gives, the
//! beacon's own period for its repeating pass, or else the interval just
//! measured. The beacon is not judged over its measured interval: the first
//! pass after a restart or a reboot measures hours, and every process new
//! since would be judged against hours while the next beacon is one period
//! away. No rate, size or share is invented here.

use std::time::Duration;

use serde::Serialize;

use super::ProcessLoad;

/// When the next reading is taken, and who said so.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Horizon {
    /// The interval just measured: the last gap between one reader's
    /// readings stands for the next one (`stado host processes`, a one-off
    /// `stado host beacon collect`).
    Measured,
    /// The seconds an operator gave (`stado host processes --within`).
    Within(Duration),
    /// The period the beacon's repeating pass takes its next reading after.
    BeaconPeriod(Duration),
}

impl Horizon {
    fn seconds(self, interval: Duration) -> f64 {
        match self {
            Horizon::Measured => interval.as_secs_f64(),
            Horizon::Within(within) | Horizon::BeaconPeriod(within) => within.as_secs_f64(),
        }
    }

    fn source(self) -> &'static str {
        match self {
            Horizon::Measured => "measured_interval",
            Horizon::Within(_) => "within",
            Horizon::BeaconPeriod(_) => "beacon_period",
        }
    }
}

/// One process whose resident memory grew in the interval.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Growth {
    pub pid: i32,
    pub program: String,
    pub resident_bytes: u64,
    /// Resident bytes gained in the interval; all of them for a process that
    /// started in it.
    pub grown_bytes: u64,
    /// At the rate it grew, when this process alone leaves no memory
    /// available.
    pub exhausts_available_in_seconds: f64,
}

/// The memory verdict of one comparison.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Runaway {
    /// Seconds until the next reading, and where that figure came from:
    /// `measured_interval`, `within` or `beacon_period`.
    pub horizon_seconds: f64,
    pub horizon_source: &'static str,
    /// What a new allocation could obtain when this reading was taken; absent
    /// with `available_unread` naming the kernel's refusal, and then nothing
    /// is judged.
    pub available_bytes: Option<i64>,
    pub available_unread: Option<String>,
    /// Every process's growth together, and when at that rate the available
    /// memory is gone; absent when nothing grew or nothing was judged.
    pub grown_bytes: u64,
    pub exhausts_available_in_seconds: Option<f64>,
    /// The fewest processes, largest growth first, whose growth together
    /// exhausts the available memory within the horizon; empty when all the
    /// growth of the interval together does not.
    pub processes: Vec<Growth>,
}

/// Seconds until `available` is gone at `grown` bytes per `interval`, for a
/// growth above zero.
fn exhaustion(available: i64, grown: u64, interval: Duration) -> f64 {
    available.max(0) as f64 * interval.as_secs_f64() / grown as f64
}

/// Judge the growth of `loads` over `interval` against `available`.
pub fn judge(
    loads: &[ProcessLoad],
    interval: Duration,
    horizon: Horizon,
    available: Result<i64, String>,
) -> Runaway {
    let horizon_seconds = horizon.seconds(interval);
    let mut growth: Vec<&ProcessLoad> = loads
        .iter()
        .filter(|load| load.resident_change_bytes > 0)
        .collect();
    growth.sort_by_key(|load| std::cmp::Reverse(load.resident_change_bytes));
    let grown_bytes: u64 = growth
        .iter()
        .map(|load| load.resident_change_bytes.unsigned_abs())
        .sum();
    let available_bytes = match available {
        Ok(bytes) => bytes,
        Err(cause) => {
            return Runaway {
                horizon_seconds,
                horizon_source: horizon.source(),
                available_bytes: None,
                available_unread: Some(cause),
                grown_bytes,
                exhausts_available_in_seconds: None,
                processes: Vec::new(),
            }
        }
    };
    let total = (grown_bytes > 0).then(|| exhaustion(available_bytes, grown_bytes, interval));
    let mut processes = Vec::new();
    if total.is_some_and(|seconds| seconds <= horizon_seconds) {
        let mut together: u64 = 0;
        for load in growth {
            let grown = load.resident_change_bytes.unsigned_abs();
            together += grown;
            processes.push(Growth {
                pid: load.pid,
                program: load.program.clone(),
                resident_bytes: load.resident_bytes,
                grown_bytes: grown,
                exhausts_available_in_seconds: exhaustion(available_bytes, grown, interval),
            });
            if exhaustion(available_bytes, together, interval) <= horizon_seconds {
                break;
            }
        }
    }
    Runaway {
        horizon_seconds,
        horizon_source: horizon.source(),
        available_bytes: Some(available_bytes),
        available_unread: None,
        grown_bytes,
        exhausts_available_in_seconds: total,
        processes,
    }
}
