//! The Weles worker workloads: capture batches, browser tasks, the activity
//! read, and the runtimes a host needs to serve them.

mod activity;
mod browser;
mod capture;
mod runtime;

pub(crate) use activity::weles_activity;
pub(crate) use browser::run_weles_browser_task;
pub(crate) use capture::{
    run_weles_capture, run_weles_diagnostics, run_weles_image_inspect, weles_capture_status,
    weles_run_diagnostics,
};
pub(crate) use runtime::{
    mobile_runtime, recordings_status, refresh_weles_api_runtime, set_weles_recordings_dir,
    weles_browser_runtime,
};
