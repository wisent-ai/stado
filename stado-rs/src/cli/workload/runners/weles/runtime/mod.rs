//! What a host needs installed, recorded and listening before it can serve a
//! Weles workload.

mod components;
mod recordings;

pub(crate) use components::{mobile_runtime, weles_browser_runtime};
pub(crate) use recordings::{recordings_status, set_weles_recordings_dir};
