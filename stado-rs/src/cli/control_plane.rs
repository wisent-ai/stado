//! `stado local-control-plane` / `stado cloud-control-plane` (both hidden).
//!
//! Python `cli.py::local_control_plane` → `deploy.local_control_plane.run`
//! and `cli.py::cloud_control_plane` → `deploy.cloud_control_plane.run`.

use super::CmdError;

pub async fn local(bind: String, port: i64, interval: i64) -> Result<(), CmdError> {
    crate::control_plane::run_local(&bind, port, interval)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))
}

pub async fn cloud(bind: String, port: i64, interval: i64) -> Result<(), CmdError> {
    crate::control_plane::run_cloud(&bind, port, interval)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))
}
