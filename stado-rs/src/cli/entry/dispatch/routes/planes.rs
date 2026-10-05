//! Where the long-running control planes land.

use crate::cli::entry::spec::root::planes::PlaneCommands;
use crate::cli::hosts::coordinator;
use crate::cli::*;

pub(crate) async fn dispatch(command: PlaneCommands) -> Result<(), CmdError> {
    match command {
        PlaneCommands::Serve(args) => crate::cli::integrations::runtime::run(*args).await,
        PlaneCommands::Coordinator { target, once } => {
            let invocation = if once {
                crate::coordinator::Invocation::Once
            } else {
                crate::coordinator::Invocation::Daemon
            };
            coordinator::run(target, invocation).await
        }
        PlaneCommands::Dashboard {
            bind,
            port,
            enrollment_only,
            inherited_listener,
        } => dashboard::run(bind, port, enrollment_only, inherited_listener).await,
    }
}
