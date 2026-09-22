//! Where the environment, endpoint and serving verbs land.

use super::super::*;

use crate::cli::service::runtime::env::set::{env_set, EnvSetOptions};
use crate::cli::service::runtime::env::show::{env_show, EnvShowOptions};
use crate::cli::service::runtime::env::unset::{env_unset, EnvUnsetOptions};
use crate::cli::service::runtime::serving::endpoint_check::{endpoint_check, EndpointCheckOptions};
use crate::cli::service::runtime::serving::report::serving;
use crate::cli::service::runtime::serving::ServingOptions;

use crate::cli::service::commands::spec::unit_inputs::environment::EnvironmentCommands;

pub(crate) async fn dispatch(command: EnvironmentCommands) -> Result<(), CmdError> {
    match command {
        EnvironmentCommands::EnvSet {
            name,
            host,
            key,
            env_file,
            value_file,
            json,
        } => {
            env_set(EnvSetOptions {
                name: &name,
                host: &host,
                key: &key,
                env_file: &env_file,
                value_file: &value_file,
                as_json: json,
            })
            .await
        }
        EnvironmentCommands::EnvUnset {
            name,
            host,
            key,
            env_file,
            json,
        } => {
            env_unset(EnvUnsetOptions {
                name: &name,
                host: &host,
                key: &key,
                env_file: &env_file,
                as_json: json,
            })
            .await
        }
        EnvironmentCommands::EnvShow {
            name,
            host,
            env_file,
            reveal,
            json,
        } => {
            env_show(EnvShowOptions {
                name: &name,
                host: &host,
                env_file: &env_file,
                reveal: reveal.as_deref(),
                as_json: json,
            })
            .await
        }
        EnvironmentCommands::EndpointCheck {
            name,
            host,
            env_file,
            json,
        } => {
            endpoint_check(EndpointCheckOptions {
                name: &name,
                host: &host,
                env_file: &env_file,
                as_json: json,
            })
            .await
        }
        EnvironmentCommands::Serving {
            name,
            host,
            ports,
            json,
        } => {
            serving(ServingOptions {
                name: &name,
                host: &host,
                ports: &ports,
                as_json: json,
            })
            .await
        }
    }
}
