//! Where the environment, endpoint and grant verbs land.

use super::*;

use crate::cli::service::runtime::env::set::{env_set, EnvSetOptions};
use crate::cli::service::runtime::env::show::{env_show, EnvShowOptions};
use crate::cli::service::runtime::env::unset::{env_unset, EnvUnsetOptions};
use crate::cli::service::runtime::secrets::auth_check::{auth_check, AuthCheckOptions};
use crate::cli::service::runtime::secrets::grant::{
    grant_sync, token_file_sync, GrantSyncOptions, TokenFileSyncOptions,
};
use crate::cli::service::runtime::serving::endpoint_check::{endpoint_check, EndpointCheckOptions};
use crate::cli::service::runtime::serving::report::serving;
use crate::cli::service::runtime::serving::ServingOptions;

use super::super::spec::environment::EnvironmentCommands;

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
        EnvironmentCommands::GrantSync {
            name,
            host,
            consumer,
            capabilities,
            vault_file,
            token_file,
            ttl_seconds,
            audience,
            json,
        } => {
            grant_sync(GrantSyncOptions {
                name: &name,
                host: &host,
                consumer: &consumer,
                capabilities: &capabilities,
                token_file: &token_file,
                vault_file: &vault_file,
                ttl_seconds,
                audience: audience.as_deref(),
                as_json: json,
            })
            .await
        }
        EnvironmentCommands::TokenFileSync {
            name,
            host,
            item,
            field,
            token_file,
            json,
        } => {
            token_file_sync(TokenFileSyncOptions {
                name: &name,
                host: &host,
                item: &item,
                field: &field,
                token_file: &token_file,
                as_json: json,
            })
            .await
        }
        EnvironmentCommands::AuthCheck {
            name,
            host,
            item,
            field,
            consumer,
            token_file,
            url,
            repair,
            take_over_listener,
            post_empty_json,
            expect_status,
            variable,
            env_file,
            json,
        } => {
            auth_check(AuthCheckOptions {
                name: &name,
                host: &host,
                item: item.as_deref(),
                field: &field,
                consumer: consumer.as_deref(),
                token_file: token_file.as_deref(),
                url: &url,
                post_empty_json,
                expect_status,
                repair,
                take_over_listener,
                variable: variable.as_deref(),
                env_file: env_file.as_deref(),
                as_json: json,
            })
            .await
        }
    }
}
