//! The queue store this deployment writes through, and the replica it is
//! recoverable from.

// ---------------------------------------------------------------------------
// 2. Storage auth + round trip
// ---------------------------------------------------------------------------

pub(in crate::doctor) mod backup;
pub(in crate::doctor) mod round_trip;
