//! Where the credential verbs land.

use super::super::*;

use crate::cli::service::runtime::secrets::auth_check::{auth_check, AuthCheckOptions};
use crate::cli::service::runtime::secrets::declared_grants::{
    declared_grant_reconcile, DeclaredGrantsOptions, GrantMint,
};
use crate::cli::service::runtime::secrets::grant::{
    grant_sync, token_file_sync, GrantSyncOptions, TokenFileSyncOptions,
};

use crate::cli::service::commands::spec::unit_inputs::credentials::{
    CredentialCommands, ServiceAuthCommands, ServiceGrantCommands, ServiceTokenFileCommands,
};

pub(crate) async fn dispatch(command: CredentialCommands) -> Result<(), CmdError> {
    match command {
        CredentialCommands::Grant { command } => grant(command).await,
        CredentialCommands::TokenFile {
            command:
                ServiceTokenFileCommands::Sync {
                    name,
                    host,
                    item,
                    field,
                    token_file,
                    json,
                },
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
        CredentialCommands::Auth {
            command:
                ServiceAuthCommands::Check {
                    name,
                    host,
                    item,
                    field,
                    url,
                    repair,
                    take_over_listener,
                    post_empty_json,
                    expect_status,
                    variable,
                    env_file,
                    json,
                },
        } => {
            auth_check(AuthCheckOptions {
                name: &name,
                host: &host,
                item: item.as_deref(),
                field: &field,
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

async fn grant(command: ServiceGrantCommands) -> Result<(), CmdError> {
    match command {
        ServiceGrantCommands::Show {
            name,
            consumer,
            json,
        } => {
            declared_grant_reconcile(DeclaredGrantsOptions {
                name: &name,
                consumer: consumer.as_deref(),
                mint: None,
                as_json: json,
            })
            .await
        }
        ServiceGrantCommands::Mint {
            name,
            consumer,
            vault_file,
            ttl_seconds,
            json,
        } => {
            declared_grant_reconcile(DeclaredGrantsOptions {
                name: &name,
                consumer: consumer.as_deref(),
                mint: Some(GrantMint {
                    vault_file: &vault_file,
                    ttl_seconds,
                }),
                as_json: json,
            })
            .await
        }
        ServiceGrantCommands::Sync {
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
    }
}
