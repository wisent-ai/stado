//! Where the long-running control planes land.

use crate::cli::entry::spec::root::planes::PlaneCommands;
use crate::cli::hosts::coordinator;
use crate::cli::integrations::control_plane;
use crate::cli::*;

pub(crate) async fn dispatch(command: PlaneCommands) -> Result<(), CmdError> {
    match command {
        PlaneCommands::Coordinator { target, once } => coordinator::run(target, once).await,
        PlaneCommands::Dashboard {
            bind,
            port,
            enrollment_only,
        } => dashboard::run(bind, port, enrollment_only).await,
        PlaneCommands::LocalControlPlane {
            bind,
            port,
            interval,
        } => control_plane::local(bind, port, interval).await,
        PlaneCommands::CloudControlPlane {
            bind,
            port,
            interval,
        } => control_plane::cloud(bind, port, interval).await,
    }
}
