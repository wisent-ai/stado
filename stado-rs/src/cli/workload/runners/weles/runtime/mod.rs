//! What a host needs installed, recorded and listening before it can serve a
//! Weles workload.

mod api;
mod components;
mod recordings;

pub(crate) use api::refresh_weles_api_runtime;
pub(crate) use components::{mobile_runtime, weles_browser_runtime};
pub(crate) use recordings::{recordings_status, set_weles_recordings_dir};
