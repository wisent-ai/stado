//! One completed convergence operation, and the vocabulary and receipts every
//! phase of it shares.

pub(in crate::cli::service_converge) mod receipts;
pub(in crate::cli::service_converge) mod vocabulary;

use serde_json::Value;

use crate::cli::service_converge::model::receipts::AppliedPass;
use crate::cli::service_converge::model::vocabulary::Row;
use crate::cli::service_converge::verdicts::reporting::gates::{apply_exit_code, report_exit_code};
use crate::cli::service_converge::verdicts::reporting::report_json;

/// One completed convergence operation.
///
/// The report is the exact object printed by `stado service converge --json`;
/// the exit code is decided alongside it from the same final rows and apply
/// receipts. A caller that receives this value therefore never has to scrape
/// process output or reproduce the command's gate.
pub struct ServiceConvergeResult {
    /// The CLI gate's exact process exit code for this completed operation.
    pub exit_code: i32,
    pub(super) target: String,
    pub(super) applied: Option<AppliedPass>,
    pub(super) rows: Vec<Row>,
}

impl ServiceConvergeResult {
    pub(super) fn new(target: String, applied: Option<AppliedPass>, rows: Vec<Row>) -> Self {
        let exit_code = match applied.as_ref() {
            Some(pass) => apply_exit_code(&rows, pass),
            None => report_exit_code(&rows),
        };
        Self {
            exit_code,
            target,
            applied,
            rows,
        }
    }

    /// The complete report shared by the CLI JSON and HTTP interfaces.
    pub fn report_json(&self) -> Value {
        report_json(&self.target, self.applied.as_ref(), &self.rows)
    }
}
