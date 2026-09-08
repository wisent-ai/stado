//! The scan budget and deadline every cleaner ticks against.

use std::time::Instant;

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;

/// Shared scan budget + deadline (Python's `budget` dict + `deadline`).
pub struct ScanBudget {
    pub remaining: i64,
    pub deadline: Instant,
}

impl ScanBudget {
    pub fn new(max_scan_items: i64, deadline: Instant) -> Self {
        Self {
            remaining: max_scan_items,
            deadline,
        }
    }

    /// Python `_hf_tick`.
    pub fn tick(&mut self, report: &mut CleanupReport) -> Result<(), JanitorError> {
        if Instant::now() >= self.deadline {
            report.caps.deadline = true;
            return Err(JanitorError::timeout("cache scan deadline"));
        }
        if self.remaining <= 0 {
            report.caps.scan = true;
            return Err(JanitorError::os("cache scan cap"));
        }
        self.remaining -= 1;
        report.hf.scanned_items += 1;
        Ok(())
    }
}
