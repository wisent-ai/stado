//! The one failure `stado build newest` exits with after some products were
//! refused or failed.
//!
//! On 2026-09-30 a batch over 14 products printed a stated cause for each of
//! nine refusals — an object API connection closed, GitHub answering an empty
//! reply, a Skarbiec 403, a changelog gate — and then ended with "the command
//! failed and we could not attribute the failure … retrying will not help"
//! (4a5a630a). The summary was built as bare prose, so the classifier read the
//! list of product names and found nothing. Each product's failure is kept
//! with the code its own error stated, and the summary states a code too: the
//! first retryable one when any refusal is worth retrying, `refused`
//! otherwise, because every product's cause is already on its own line.

use crate::cli::CmdError;
use crate::primitives::failure::{classify_message, FailureCode};

/// One product that did not build: its name, the code its error stated where
/// it was built, and the sentence printed for it.
pub(crate) struct Refusal<'a> {
    pub product: &'a str,
    pub code: Option<FailureCode>,
    pub failure: Option<&'a str>,
}

impl Refusal<'_> {
    /// The stated code, or the wording classifier's reading of the printed
    /// sentence when the error arrived as prose (the documented last resort).
    fn code(&self) -> FailureCode {
        self.code
            .or_else(|| self.failure.map(classify_message))
            .unwrap_or(FailureCode::Unknown)
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
