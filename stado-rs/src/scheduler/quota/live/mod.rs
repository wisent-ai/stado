//! The live cloud limit readers, one per provider quota API: `gcp` sums the
//! regional NVIDIA_*_GPUS metrics the Compute REST API reports (and resolves
//! the project that read targets), `azure` converts the regional
//! Microsoft.Compute vCPU-family limits ARM reports into schedulable GPU
//! slots. Both return a limit map keyed by internal accel_type names; the
//! reservation overlay is composed on top of it in `super::compose`.

pub(super) mod azure;
pub(super) mod gcp;
