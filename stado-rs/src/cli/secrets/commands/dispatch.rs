//! One match from a parsed verb to the function that answers it.

use crate::cli::CmdError;

use crate::cli::secrets::commands::subcommands::{
    CredentialAcquisitionScopeCommands, CredentialBackupCommands, CredentialGrantCommands,
    CredentialItemCommands, CredentialTokenCommands, CredentialVaultCommands,
};
use crate::cli::secrets::commands::surface::{
    CredentialSeedCommands, CredentialSigningProfileCommands, CredentialSparkleKeyCommands,
    SecretsCommands,
};
use crate::cli::secrets::diagnostics::doctor::{doctor, vault_authority};
use crate::cli::secrets::diagnostics::harvest::harvest;
use crate::cli::secrets::diagnostics::unlock::try_unlock;
use crate::cli::secrets::store::grants::migrate;
use crate::cli::secrets::store::inventory::{inspect_host_vault, inspect_vault};
use crate::cli::secrets::store::items::{
    get, item_in_role, ls, put, put_role, rm, rotate, store, Store,
};

pub async fn dispatch(command: SecretsCommands) -> Result<(), CmdError> {
    match command {
        // `doctor` is answered before any client exists. Every other verb needs
        // a grant, a token and a live service, which is exactly the set of
        // things this verb is for when one of them is what broke.
        SecretsCommands::Doctor { json } => doctor(json),
        SecretsCommands::Vault { command } => match command {
            CredentialVaultCommands::Show { json } => vault_authority(json).await,
            CredentialVaultCommands::List { host, json } => super::host::vaults(host, json).await,
            CredentialVaultCommands::Items {
                vault,
                host,
                matching,
                json,
            } => match (host, vault) {
                (Some(host), None) => inspect_host_vault(&host, matching.as_deref(), json).await,
                (None, Some(vault)) => inspect_vault(&vault, matching.as_deref(), json),
                (Some(_), Some(_)) => Err(CmdError::usage(
                    "vault items reads either a local VAULT file or --host, not both",
                )),
                (None, None) => Err(CmdError::usage(
                    "vault items needs a local VAULT file or --host",
                )),
            },
            CredentialVaultCommands::Sync {
                host,
                check,
                push,
                json,
            } => {
                if push {
                    super::host::push_vault(&host, json).await
                } else {
                    super::host::sync_vault(&host, check, json).await
                }
            }
            CredentialVaultCommands::Retire {
                path,
                owner,
                apply,
                json,
            } => crate::cli::secrets::retire::retire_copy(&path, &owner, apply, json).await,
        },
        // Same reasoning as `doctor`: the transcripts are readable when the
        // vault is not, which is the only reason this verb is worth having.
        SecretsCommands::Harvest { json, restore, all } => {
            harvest(json, restore.as_deref(), all).await
        }
        // Also answered without a client: a protected key that nothing can
        // unlock is precisely the state where every other verb is unavailable.
        SecretsCommands::TryUnlock {
            host,
            keychain_only,
        } => try_unlock(host.as_deref(), keychain_only).await,
        SecretsCommands::Migrate { to } => migrate(to.as_deref()).await,
        SecretsCommands::Put {
            name,
            role,
            item_type,
            field,
            route,
            consumer,
            grant_file,
        } => match (route.is_some(), field) {
            (true, Some(field)) => {
                let Some(name) = name else {
                    return Err(CmdError::usage("--route writes the item it names"));
                };
                let Store::Skarbiec(client) = delegated_or_selected(route, consumer, grant_file)?
                else {
                    return Err(CmdError::usage(
                        "--route writes through a Skarbiec consumer grant",
                    ));
                };
                rotate(&client, &name, &field).await
            }
            (true, None) => Err(CmdError::usage("--route needs --field")),
            (false, _) => match (name, role) {
                (Some(name), None) => put(&store()?, &name, item_type.as_deref()).await,
                (None, Some(role)) => put_role(&role, item_type.as_deref()).await,
                _ => Err(CmdError::usage(
                    "credentials put names either an item id or --role, not both",
                )),
            },
        },
        SecretsCommands::Get {
            name,
            role,
            field,
            route,
            consumer,
            grant_file,
        } => {
            // A delegated reader holds a grant for its fields, not the right to
            // list the vault, so it cannot look the role up itself: it asks
            // Skarbiec for the coordinate `role:<role>`, which a
            // `read:role:<role>#<field>` grant authorizes and Skarbiec resolves
            // to the one live item playing the role. The store administrator
            // still finds the item by listing.
            let delegated = route.is_some();
            let selected = delegated_or_selected(route, consumer, grant_file)?;
            let name = match (name, role) {
                (Some(name), None) => name,
                (None, Some(role)) if delegated => format!("role:{role}"),
                (None, Some(role)) => item_in_role(&selected, &role).await?,
                _ => {
                    return Err(CmdError::usage(
                        "credentials get names either an item id or --role, not both",
                    ))
                }
            };
            get(&selected, &name, field.as_deref()).await
        }
        SecretsCommands::Ls { json } => ls(&store()?, json).await,
        SecretsCommands::Rm { name } => rm(&store()?, &name).await,
        SecretsCommands::Item { command } => match command {
            CredentialItemCommands::Put {
                host,
                role,
                item_type,
                json,
            } => super::host::vault_item_put(&host, &role, &item_type, json).await,
            CredentialItemCommands::Show {
                host,
                item,
                field,
                json,
            } => super::host::vault_item_show(&host, &item, field.as_deref(), json).await,
            CredentialItemCommands::Retag {
                host,
                item,
                tags,
                json,
            } => super::host::retag_vault_item(&host, &item, tags.as_deref(), json).await,
            CredentialItemCommands::Rename {
                host,
                from,
                to,
                json,
            } => super::host::rename_vault_item(&host, &from, &to, json).await,
            CredentialItemCommands::Delete { host, item, json } => {
                super::host::delete_vault_item(&host, &item, json).await
            }
            CredentialItemCommands::Restore { host, item, json } => {
                super::host::restore_vault_item(&host, &item, json).await
            }
            CredentialItemCommands::Reclaim { host, item, json } => {
                super::host::reclaim_vault_item(&host, &item, json).await
            }
            CredentialItemCommands::Upgrade { host, apply, json } => {
                super::host::upgrade_vault(&host, apply, json).await
            }
            CredentialItemCommands::SummarizeLocal => super::host::summarize_item_local(),
        },
        SecretsCommands::Token { command } => match command {
            CredentialTokenCommands::Mint {
                host,
                consumer,
                capabilities,
                audience,
                ttl_seconds,
                replace_capabilities,
                token_item,
                token_field,
                raw_token,
                token_file_name,
                store_item,
                json,
            } => {
                super::host::vault_token_mint(
                    &host,
                    &consumer,
                    &capabilities,
                    &audience,
                    ttl_seconds,
                    replace_capabilities,
                    token_item.as_deref(),
                    token_field.as_deref().unwrap_or("token"),
                    raw_token,
                    token_file_name.as_deref(),
                    store_item.as_deref(),
                    json,
                )
                .await
            }
            CredentialTokenCommands::Sync {
                consumer,
                from_host,
                host,
                source_token_file,
                token_file,
                check,
                shared_vault,
                json,
            } => {
                super::host::vault_token_sync(
                    &from_host,
                    &host,
                    &consumer,
                    &source_token_file,
                    &token_file,
                    super::host::TokenSyncMode::from_flags(check, shared_vault),
                    json,
                )
                .await
            }
            CredentialTokenCommands::RegisterItemLocal {
                skarbiec,
                item,
                field,
                arguments,
            } => super::host::register_item_local(&skarbiec, &item, &field, &arguments),
            CredentialTokenCommands::CustodyLocal {
                operation,
                vault,
                consumer,
                file,
            } => super::host::custody_local(&operation, &vault, &consumer, &file),
        },
        SecretsCommands::AcquisitionScopes { command } => match command {
            CredentialAcquisitionScopeCommands::Sync {
                host,
                source,
                ttl_seconds,
            } => super::host::sync_acquisition_scopes(&host, &source, ttl_seconds).await,
        },
        SecretsCommands::Grant { command } => match command {
            CredentialGrantCommands::Add {
                host,
                consumer,
                role,
                field,
                token_file,
                json,
            } => {
                super::host::grant_item_read(&host, &consumer, &role, &field, &token_file, json)
                    .await
            }
            CredentialGrantCommands::Consolidate {
                host,
                sources,
                token_file,
                json,
            } => super::host::consolidate_grants(&host, &sources, &token_file, json).await,
            CredentialGrantCommands::Rebind {
                host,
                token_file,
                json,
            } => super::host::rebind_grant(&host, &token_file, json).await,
            CredentialGrantCommands::Revoke {
                host,
                consumer,
                json,
            } => super::host::revoke_retired(&host, &consumer, json).await,
            CredentialGrantCommands::Show {
                host,
                consumer,
                token_file,
                json,
            } => super::host::grant_show(&host, &consumer, token_file.as_deref(), json).await,
            CredentialGrantCommands::Renew { force, json } => {
                use crate::providers::local::agent::tick::gates::grant::{renew, RenewOutcome};
                let mut lines = Vec::new();
                let outcome = renew(force, &mut |line| lines.push(line.to_string())).await;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "renewed": outcome == RenewOutcome::Renewed,
                            "failed": outcome == RenewOutcome::Failed,
                            "lines": lines,
                        })
                    );
                } else {
                    for line in &lines {
                        println!("{line}");
                    }
                }
                if outcome == RenewOutcome::Failed {
                    return Err(CmdError::click(format!(
                        "agent grant renewal failed: {}",
                        lines
                            .last()
                            .map(String::as_str)
                            .unwrap_or("no step reported why")
                    ))
                    .stating(crate::primitives::failure::FailureCode::InfraDown));
                }
                Ok(())
            }
        },
        SecretsCommands::Backup { command } => match command {
            CredentialBackupCommands::Audit {
                host,
                objects,
                inventory_namespaces,
                reclaim_twins,
                apply,
                json,
            } => {
                super::host::backup_audit(
                    &host,
                    &objects,
                    &inventory_namespaces,
                    reclaim_twins,
                    apply,
                    json,
                )
                .await
            }
        },
        SecretsCommands::Seed { command } => match command {
            CredentialSeedCommands::List {
                host,
                login_item,
                json,
            } => {
                super::seed_freshness::authenticator_seed_freshness(
                    &host,
                    login_item.as_deref(),
                    json,
                )
                .await
            }
            CredentialSeedCommands::Enrol {
                host,
                login_item,
                json,
            } => crate::cli::seed_enrol::enrol_authenticator_seed(&host, &login_item, json).await,
        },
        SecretsCommands::SparkleKey {
            command:
                CredentialSparkleKeyCommands::Mint {
                    product,
                    info_plist,
                    replace,
                    json,
                },
        } => {
            crate::cli::secrets::store::sparkle::sparkle_key(&product, &info_plist, replace, json)
                .await
        }
        SecretsCommands::SigningProfile {
            command: CredentialSigningProfileCommands::Ensure(args),
        } => super::host::apple_profile(args).await,
    }
}

/// The store a delegated `get` or `put` acts on: a Skarbiec client under the
/// named consumer's own grant when `--route` is given, else the selected store.
fn delegated_or_selected(
    route: Option<String>,
    consumer: Option<String>,
    grant_file: Option<String>,
) -> Result<Store, CmdError> {
    let Some(route) = route else {
        return store();
    };
    let (Some(consumer), Some(grant_file)) = (consumer, grant_file) else {
        return Err(CmdError::usage(
            "--route needs both --consumer and --grant-file",
        ));
    };
    crate::skarbiec::Client::new(
        &route,
        &consumer,
        &grant_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .map(Store::Skarbiec)
    .map_err(CmdError::from)
}
