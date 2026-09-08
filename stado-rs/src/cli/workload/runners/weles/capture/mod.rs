//! The `weles-capture`, `weles-diagnostics` and `weles-image-inspect`
//! workloads, all of which speak to a host's Weles admission endpoint.

mod diagnostics;
mod enqueue;
mod inspect;
mod status;

pub(crate) use diagnostics::{run_weles_diagnostics, weles_run_diagnostics};
pub(crate) use enqueue::run_weles_capture;
pub(crate) use inspect::run_weles_image_inspect;
pub(crate) use status::weles_capture_status;
