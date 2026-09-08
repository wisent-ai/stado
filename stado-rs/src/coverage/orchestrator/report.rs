//! What one coverage walk found.

use serde_json::{json, Value};

use crate::coverage::UniverseEntry;

/// Python `CoverageReport` dataclass.
#[derive(Debug, Clone)]
pub struct CoverageReport {
    pub universe_id: String,
    pub total_entries: usize,
    pub present: usize,
    pub missing: usize,
    pub unfixable: Vec<(String, String)>,
    pub gaps: Vec<UniverseEntry>,
    pub opaque: Vec<UniverseEntry>,
}

impl CoverageReport {
    /// Python `CoverageReport.as_dict()` (key order is the Python dict
    /// literal order — the CLI prints it unsorted).
    pub fn as_dict(&self) -> Value {
        json!({
            "universe_id": self.universe_id,
            "total_entries": self.total_entries,
            "present": self.present,
            "missing": self.missing,
            "unfixable_count": self.unfixable.len(),
            "gap_count": self.gaps.len(),
            "opaque_count": self.opaque.len(),
        })
    }
}
