//! Per-job measurement primitives shared by both collectors: the
//! wall-clock span a job occupied, the catalog hourly rate that span is
//! priced at, and the provider/model attribution its cost is bucketed by.

pub(super) mod attribution;
pub(super) mod rates;
pub(super) mod timing;
