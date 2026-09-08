//! One match from a parsed verb to the function that answers it.

use crate::cli::CmdError;

use crate::cli::secrets::commands::subcommands::{
    CredentialAcquisitionScopeCommands, CredentialBackupCommands, CredentialGrantCommands,
    CredentialItemCommands, CredentialTokenCommands, CredentialVaultCommands,
};
use crate::cli::secrets::commands::surface::SecretsCommands;
use crate::cli::secrets::diagnostics::doctor::{doctor, vault_authority};
use crate::cli::secrets::diagnostics::harvest::harvest;
use crate::cli::secrets::diagnostics::unlock::try_unlock;
use crate::cli::secrets::store::grants::{migrate, mint_acquisition_token};
use crate::cli::secrets::store::inventory::{inspect_host_vault, inspect_vault};
use crate::cli::secrets::store::items::{get, ls, put, rm};
use crate::cli::secrets::store::resolve::client;
use crate::cli::secrets::weles::adopt::adopt_weles_vault;
use crate::cli::secrets::weles::bootstrap::bootstrap_weles;

pub async fn dispatch(command: SecretsCommands) -> Result<(), CmdError> {
    match command {
        // `doctor` is answered before any client exists. Every other verb needs
        // a grant, a token and a live service, which is exactly the set of
        // things this verb is for when one of them is what broke.
        SecretsCommands::Doctor { json } => doctor(json),
        SecretsCommands::Vault { command, json } => match command {
            None => vault_authority(json),
            Some(CredentialVaultCommands::Sync { host, check, json }) => {
                super::host::sync_vault(&host, check, json).await
            }
        },
        SecretsCommands::InspectVault {
            vault,
            host,
            matching,
            json,
        } => match (host, vault) {
            (Some(host), None) => inspect_host_vault(&host, matching.as_deref(), json).await,
            (None, Some(vault)) => inspect_vault(&vault, json),
            (Some(_), Some(_)) => Err(CmdError::usage(
                "inspect-vault reads either a local VAULT file or --host, not both",
            )),
            (None, None) => Err(CmdError::usage(
                "inspect-vault needs a local VAULT file or --host",
            )),
        },
        SecretsCommands::BootstrapWeles { json } => bootstrap_weles(json),
        SecretsCommands::AdoptWelesVault { json } => adopt_weles_vault(json),
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
        SecretsCommands::Put { name, item_type } => {
            put(&client()?, &name, item_type.as_deref()).await
        }
        SecretsCommands::Get { name, field } => get(&client()?, &name, field.as_deref()).await,
        SecretsCommands::Ls { json } => ls(&client()?, json).await,
        SecretsCommands::Rm { name } => rm(&client()?, &name).await,
        SecretsCommands::MintAcquisitionToken {
            consumer,
            item,
            field,
            output,
        } => mint_acquisition_token(&consumer, &item, &field, &output),
        SecretsCommands::Item { command } => match command {
            CredentialItemCommands::Put {
                host,
                item,
                item_type,
                json,
            } => super::host::vault_item_put(&host, &item, &item_type, json).await,
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
                    json,
                )
                .await
            }
        },
        SecretsCommands::Vaults { host, json } => super::host::vaults(host, json).await,
        SecretsCommands::AcquisitionScopes { command } => match command {
            CredentialAcquisitionScopeCommands::Sync { host, source } => {
                super::host::sync_acquisition_scopes(&host, &source).await
            }
        },
        SecretsCommands::Grant { command } => match command {
            CredentialGrantCommands::ItemRead {
                host,
                consumer,
                item,
                field,
                token_file,
                json,
            } => {
                super::host::grant_item_read(&host, &consumer, &item, &field, &token_file, json)
                    .await
            }
            CredentialGrantCommands::Show {
                host,
                consumer,
                token_file,
                json,
            } => super::host::grant_show(&host, &consumer, token_file.as_deref(), json).await,
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
        SecretsCommands::SeedFreshness {
            host,
            login_item,
            json,
        } => {
            super::seed_freshness::authenticator_seed_freshness(&host, login_item.as_deref(), json)
                .await
        }
    }
}
