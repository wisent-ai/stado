//! The one failure `stado build newest` exits with after some products were
//! refused or failed.
//!
//! A batch that prints a stated cause for each refusal — an object API
//! connection closed, GitHub answering an empty reply, a Skarbiec 403, a
//! changelog gate — must not end with a summary that attributes nothing.
//! Each product's failure is kept with the code its own error stated, and
//! the summary states a code too: the first retryable one when any refusal
//! is worth retrying, `refused` otherwise, because every product's cause is
//! already on its own line.

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// One product that did not build: its name and the code its error stated
/// where it was built.
pub(crate) struct Refusal<'a> {
    pub product: &'a str,
    pub code: Option<FailureCode>,
}

impl Refusal<'_> {
    /// The stated code; an error that arrived as prose and stated none is
    /// `unknown`, never a reading of its wording.
    fn code(&self) -> FailureCode {
        self.code.unwrap_or(FailureCode::Unknown)
    }
}

/// The batch's failure: every product named with its own class.
pub(crate) fn batch_failure(refusals: &[Refusal<'_>]) -> CmdError {
    let codes: Vec<FailureCode> = refusals.iter().map(Refusal::code).collect();
    let listed = refusals
        .iter()
        .zip(&codes)
        .map(|(refusal, code)| format!("{} ({})", refusal.product, code.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    let stated = codes
        .iter()
        .find(|code| code.retryable())
        .cloned()
        .unwrap_or(FailureCode::Refused);
    CmdError::click(format!(
        "build newest: {} product(s) failed or refused, each cause printed above: {listed}",
        refusals.len()
    ))
    .stating(stated)
}
