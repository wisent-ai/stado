//! Where the long-running host process lands.

use crate::cli::entry::spec::root::planes::PlaneCommands;
use crate::cli::*;

pub(crate) async fn dispatch(command: PlaneCommands) -> Result<(), CmdError> {
    match command {
        PlaneCommands::Serve(args) => crate::cli::integrations::runtime::run(*args).await,
    }
}
