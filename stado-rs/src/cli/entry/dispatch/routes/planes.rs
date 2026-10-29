//! Where the long-running host process lands.

use crate::cli::entry::spec::root::planes::PlaneCommands;
use crate::cli::*;

pub(crate) async fn dispatch(command: PlaneCommands) -> Result<(), CmdError> {
    match command {
        PlaneCommands::Serve(args) => match crate::cli::integrations::runtime::run(*args).await {
            Ok(crate::cli::integrations::runtime::Served::Finished) => Ok(()),
            // Product sync installed the release that replaces this one: the
            // process ends with the retry exit code, which launchd's
            // KeepAlive and systemd's Restart=on-failure both answer by
            // starting the installed release, and it is not a stop of this
            // release, so nothing is kept as one.
            Ok(crate::cli::integrations::runtime::Served::Replaced) => Err(CmdError::silent(
                crate::primitives::failure::FailureCode::RETRY_EXIT,
            )),
            // A release that stops again on this host is recorded as its
            // failed install (`runtime::stops`).
            Err(failure) => {
                crate::cli::integrations::runtime::stops::record(&failure);
                Err(failure)
            }
        },
        PlaneCommands::Network(command) => crate::cli::network::dispatch(command).await,
    }
}
