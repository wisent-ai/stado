//! The adapter-run report record family: [`VerificationReport`].

use serde_json::{Map, Value};

/// Result of one verification adapter run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerificationReport {
    pub adapter: String,
    pub passed: bool,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub summary: Map<String, Value>,
}
