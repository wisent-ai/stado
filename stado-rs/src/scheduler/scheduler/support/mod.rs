//! Pass-independent scheduler helpers: Python-format stderr reporting,
//! accelerator hourly rates, and the dispatch pacing controls. Kept beside
//! the passes rather than inside one of them because agent-VM bucketing
//! outside this tree reads the rate and the backoff window too.

pub(super) mod pacing;
pub(super) mod rates;
pub(super) mod reporting;
