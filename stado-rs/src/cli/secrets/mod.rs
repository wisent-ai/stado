//! `stado secrets` — operator surface for the selected credential store.
//!
//! Secret values travel in request bodies, never argv. The backend selected by
//! `STADO_CREDENTIALS_STORE` owns every item; changing it is completed through
//! the verified `migrate` command before normal credential access resumes.

pub(in crate::cli::secrets) mod commands;
pub(in crate::cli::secrets) mod diagnostics;
pub(in crate::cli::secrets) mod store;
pub(in crate::cli::secrets) mod weles;

pub use crate::cli::secrets::commands::dispatch::dispatch;
pub use crate::cli::secrets::commands::subcommands::{
    CredentialAcquisitionScopeCommands, CredentialBackupCommands, CredentialGrantCommands,
    CredentialItemCommands, CredentialTokenCommands, CredentialVaultCommands,
};
pub use crate::cli::secrets::commands::surface::SecretsCommands;
/// The local Skarbiec launcher and its JSON reports, for the one place
/// outside this surface that issues a grant: the agent renewing its own.
pub(crate) use crate::cli::secrets::store::resolve::{launcher_json, skarbiec_launcher};
