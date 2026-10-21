//! What one advisory pass published, by recommendation kind: the resources
//! each kind of advice was written for.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvisorSummary {
    pub rightsizing: Vec<String>,
    pub schedules: Vec<String>,
    pub storage_lifecycle: Vec<String>,
    pub network: Vec<String>,
    pub commitments: Vec<String>,
    /// Each resource the pass could not judge, with why: advice it may have
    /// earned is not published, and saying so keeps it from reading as none.
    #[serde(default)]
    pub unjudged: Vec<String>,
}

impl AdvisorSummary {
    /// Whether the pass published any advice at all.
    pub fn is_empty(&self) -> bool {
        self.rightsizing.is_empty()
            && self.schedules.is_empty()
            && self.storage_lifecycle.is_empty()
            && self.network.is_empty()
            && self.commitments.is_empty()
    }
}
