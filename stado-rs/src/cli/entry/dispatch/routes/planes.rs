//! Where the long-running host process lands.

use crate::cli::entry::spec::root::planes::PlaneCommands;
use crate::cli::*;

pub(crate) async fn dispatch(command: PlaneCommands) -> Result<(), CmdError> {
    match command {
        PlaneCommands::Serve(args) => {
            // A release that stops again on this host is recorded as its
            // failed install (`runtime::stops`).
            let served = crate::cli::integrations::runtime::run(*args).await;
            if let Err(failure) = &served {
                crate::cli::integrations::runtime::stops::record(failure);
            }
            served
        }
    }
}
