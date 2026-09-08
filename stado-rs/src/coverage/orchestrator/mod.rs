// ---------------------------------------------------------------------------
// verify / retry orchestrator
// ---------------------------------------------------------------------------

mod report;
mod retry;
mod state;
mod walk;

pub use report::CoverageReport;
pub use retry::{retry_gaps, verify_and_retry, verify_and_retry_with_store};
pub use state::{state_load, state_save};
pub use walk::verify;

pub(in crate::coverage) use state::state_slot;
