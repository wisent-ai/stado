//! Per-job measurement primitives: the wall-clock span a job occupied and
//! the provider/model attribution its cost is bucketed by.

pub(super) mod attribution;
pub(super) mod timing;
