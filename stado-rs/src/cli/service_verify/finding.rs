//! One row of the report: what a declaration is, and how it prints.

use serde_json::{json, Value};

/// What a standby row says in place of a verdict, in the words it prints.
///
/// Spelled once, because it is also the wire text an operator greps for and
/// the sentence that has to keep a reader from reading a blank probe column
/// as a failure.
pub(in crate::cli::service_verify) const STANDBY_DETAIL: &str =
    "standby address for a host that is not serving; not probed";

/// One declaration, checked or explicitly not checked.
pub(crate) struct Finding {
    pub(crate) service: String,
    pub(crate) host: String,
    pub(crate) endpoint: String,
    pub(crate) state: &'static str,
    pub(crate) detail: String,
    /// Did anything go and look? False only for a standby address, which is
    /// declared not to be serving and so has nothing to answer for.
    ///
    /// A flag rather than a comparison against [`STANDBY_DETAIL`]: the sweep
    /// and observation writer must agree without matching human prose.
    pub(crate) probed: bool,
}

impl Finding {
    fn to_json(&self) -> Value {
        json!({
            "service": self.service,
            "host": self.host,
            "endpoint": self.endpoint,
            "state": self.state,
            "detail": self.detail,
            "probed": self.probed,
        })
    }
}

pub(in crate::cli::service_verify) fn emit(findings: &[Finding], json_output: bool) {
    if json_output {
        let rows: Vec<Value> = findings.iter().map(Finding::to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
        );
        return;
    }
    println!(
        "{:<22} {:<20} {:<34} {:<12} DETAIL",
        "SERVICE", "HOST", "ENDPOINT", "STATE"
    );
    for finding in findings {
        println!(
            "{:<22} {:<20} {:<34} {:<12} {}",
            finding.service, finding.host, finding.endpoint, finding.state, finding.detail
        );
    }
}
