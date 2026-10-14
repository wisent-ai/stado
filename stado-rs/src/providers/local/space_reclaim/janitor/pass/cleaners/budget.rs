//! The count of filesystem entries the HuggingFace cleaner visited.

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;

/// Counts every entry the HuggingFace inventory and its rechecks visit into
/// `report.hf.scanned_items`. There is no limit: under the disk-full rule a
/// pass visits everything.
pub struct ScanCount;

impl ScanCount {
    /// Count one visited entry.
    pub fn tick(&mut self, report: &mut CleanupReport) -> Result<(), JanitorError> {
        report.hf.scanned_items += 1;
        Ok(())
    }
}
