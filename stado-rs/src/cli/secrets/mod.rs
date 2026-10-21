//! `stado credentials` — operator surface for the selected credential store.
//!
//! Secret values travel in request bodies, never argv. The backend selected by
//! `STADO_CREDENTIALS_STORE` owns every item; changing it is completed through
//! the verified `migrate` command before normal credential access resumes.

pub(in crate::cli::secrets) mod commands;
pub(in crate::cli::secrets) mod diagnostics;
pub(in crate::cli::secrets) mod retire;
pub(in crate::cli::secrets) mod run;
pub(in crate::cli::secrets) mod store;

pub use crate::cli::secrets::commands::dispatch::dispatch;
pub use crate::cli::secrets::commands::subcommands::{
    CredentialAcquisitionScopeCommands, CredentialBackupCommands, CredentialGrantCommands,
    CredentialItemCommands, CredentialTokenCommands, CredentialVaultCommands,
};
pub use crate::cli::secrets::commands::surface::SecretsCommands;
/// The selected store and its role lookup, for a quality check that hands a
/// gate the `secret_env` values the release worker would resolve.
pub(crate) use crate::cli::secrets::store::items::{item_in_role, store, Store};
/// The local Skarbiec launcher and its JSON reports, for the one place
/// outside this surface that issues a grant: the agent renewing its own.
pub(crate) use crate::cli::secrets::store::resolve::{launcher_json, skarbiec_launcher};
