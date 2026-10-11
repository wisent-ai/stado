//! The verbs that act on one host's vault — the vault itself, its items, its
//! backups and the authenticator seeds it holds — each answered by the
//! function that does the work.

use crate::cli::secrets::commands::subcommands::{
    CredentialBackupCommands, CredentialItemCommands, CredentialVaultCommands,
};
use crate::cli::secrets::commands::surface::CredentialSeedCommands;
use crate::cli::secrets::diagnostics::doctor::vault_authority;
use crate::cli::secrets::store::inventory::{inspect_host_vault, inspect_vault};
use crate::cli::CmdError;

pub(super) async fn vault(command: CredentialVaultCommands) -> Result<(), CmdError> {
    match command {
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
    }
}

pub(super) async fn item(command: CredentialItemCommands) -> Result<(), CmdError> {
    match command {
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
    }
}

pub(super) async fn backup(command: CredentialBackupCommands) -> Result<(), CmdError> {
    match command {
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
    }
}

pub(super) async fn seed(command: CredentialSeedCommands) -> Result<(), CmdError> {
    match command {
        CredentialSeedCommands::List {
            host,
            login_item,
            json,
        } => {
            super::seed_freshness::authenticator_seed_freshness(&host, login_item.as_deref(), json)
                .await
        }
        CredentialSeedCommands::Enrol {
            host,
            login_item,
            json,
        } => crate::cli::seed_enrol::enrol_authenticator_seed(&host, &login_item, json).await,
    }
}
