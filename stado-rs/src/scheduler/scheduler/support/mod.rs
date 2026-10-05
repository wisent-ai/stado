//! Pass-independent scheduler helpers: Python-format stderr reporting and
//! accelerator hourly rates. Kept beside the passes rather than inside one of
//! them because agent-VM bucketing outside this tree reads the rate too.

pub(super) mod rates;
pub(super) mod reporting;
