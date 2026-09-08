//! One runner per declared workload kind. Nothing here reads the
//! declaration; `super::commands` picks the runner a kind names.

mod gui;
mod jeden;
mod weles;

pub(crate) use gui::{gui_automation_status, run_gui_automation};
pub(crate) use jeden::{connect_jeden, current_workspace};
pub(crate) use weles::{
    mobile_runtime, recordings_status, refresh_weles_api_runtime, run_weles_browser_task,
    run_weles_capture, run_weles_diagnostics, run_weles_image_inspect, set_weles_recordings_dir,
    weles_activity, weles_browser_runtime, weles_capture_status, weles_run_diagnostics,
};
