//! The managed services around the compute fleet: the project and IAM footing
//! it stands on, the Cloud Run coordinator, and the schedulers, functions,
//! service accounts and builds that feed it.

pub(super) mod cloud_run;
pub(super) mod managed;
pub(super) mod platform;
