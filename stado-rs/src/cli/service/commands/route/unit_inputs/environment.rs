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
        EnvironmentCommands::UnitEnvLocal {
            path_b64,
            key_b64,
            value_stdin,
            uid,
        } => {
            // The value arrives base64-encoded on standard input, never in argv.
            let value_b64 = if value_stdin {
                let text = std::io::read_to_string(std::io::stdin()).map_err(|error| {
                    CmdError::click(format!(
                        "cannot read the value from standard input: {error}"
                    ))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
                })?;
                Some(text.trim().to_string())
            } else {
                None
            };
            crate::deploy::service::unit_env_local(&path_b64, &key_b64, value_b64.as_deref(), uid);
            Ok(())
        }
    }
}
