//! The `stado service` command line: the verbs, and where each one lands.
//!
//! [`ServiceCommands`] carries the four declaration blocks of [`spec`] with
//! `#[command(flatten)]`, in the order they were declared. `clap` inserts a
//! flattened enum's variants at the position of the variant that flattens it,
//! so this enum accepts exactly the command lines — and prints exactly the
//! help — that one undivided enum did.

use super::*;

use clap::Subcommand;

pub mod spec;

mod route;

#[derive(Subcommand)]
pub enum ServiceCommands {
    #[command(flatten)]
    Read(spec::read::ReadCommands),
    #[command(flatten)]
    Runtime(spec::runtime::RuntimeCommands),
    #[command(flatten)]
    Environment(spec::unit_inputs::environment::EnvironmentCommands),
    #[command(flatten)]
    Credentials(spec::unit_inputs::credentials::CredentialCommands),
    #[command(flatten)]
    Lifecycle(spec::lifecycle::LifecycleCommands),
}

pub async fn dispatch(command: ServiceCommands) -> Result<(), CmdError> {
    match command {
        ServiceCommands::Read(command) => route::read::dispatch(command).await,
        ServiceCommands::Runtime(command) => route::runtime::dispatch(command).await,
        ServiceCommands::Environment(command) => {
            route::unit_inputs::environment::dispatch(command).await
        }
        ServiceCommands::Credentials(command) => {
            route::unit_inputs::credentials::dispatch(command).await
        }
        ServiceCommands::Lifecycle(command) => route::lifecycle::dispatch(command).await,
    }
}
