//! The shared count of filesystem entries left for a cleaner.

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;

/// Shared scan capacity.
pub struct ScanBudget {
    pub remaining: i64,
}

impl ScanBudget {
    pub fn new(max_scan_items: i64) -> Self {
        Self {
            remaining: max_scan_items,
        }
    }

    /// Python `_hf_tick`.
    pub fn tick(&mut self, report: &mut CleanupReport) -> Result<(), JanitorError> {
        if self.remaining <= 0 {
            report.caps.scan = true;
            return Err(JanitorError::os("cache scan cap"));
        }
        self.remaining -= 1;
        report.hf.scanned_items += 1;
        Ok(())
    }
}
