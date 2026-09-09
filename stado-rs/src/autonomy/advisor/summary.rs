//! What one advisory pass counted, by recommendation kind.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvisorSummary {
    pub rightsizing: usize,
    pub schedules: usize,
    pub storage_lifecycle: usize,
    pub network: usize,
    pub commitments: usize,
}
