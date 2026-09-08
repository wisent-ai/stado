//! `stado artifact ...` — port of the artifact command group in
//! `stado/cli.py` (import/list/show/resolve/publish/alias/verify/lineage).
//!
//! Output formats match the click implementation: `--json` is
//! `json.dumps(value, indent=2, sort_keys=True)` except `resolve` and
//! `alias set`, which use Python's default (", " / ": ") separators;
//! errors surface as click `ClickException`s ("Error: {CODE}: {message}",
//! exit 1), and a failed `verify` exits 1 after printing the report.

mod format;
mod reads;
mod verification;
mod writes;

use crate::artifacts::registry::{ArtifactRegistry, RegistryError};

use super::{ArtifactAliasCommands, ArtifactCommands, ArtifactImportCommands, CmdError};

use self::reads::{lineage, list, resolve, show};
use self::verification::verify;
use self::writes::{alias_set, import_activations, publish};

/// Python `_artifact_call`: ArtifactError → `Error: {code}: {message}`
/// (exit 1); storage failures print their bare message.
fn artifact_error(exc: RegistryError) -> CmdError {
    match exc {
        RegistryError::Artifact(err) => CmdError::click(format!("{}: {}", err.code, err.message)),
        RegistryError::Storage(err) => CmdError::click(err.to_string()),
    }
}

impl From<RegistryError> for CmdError {
    fn from(exc: RegistryError) -> Self {
        artifact_error(exc)
    }
}

impl From<crate::artifacts_models::ArtifactError> for CmdError {
    fn from(err: crate::artifacts_models::ArtifactError) -> Self {
        artifact_error(err.into())
    }
}

async fn registry() -> Result<ArtifactRegistry, CmdError> {
    ArtifactRegistry::new()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))
}

pub(super) async fn dispatch(sub: ArtifactCommands) -> Result<(), CmdError> {
    match sub {
        ArtifactCommands::Import(ArtifactImportCommands::Activations {
            repo,
            revision,
            desired_state_dir,
            run_id,
            job_ids,
            version,
            alias,
            full,
            json,
        }) => {
            import_activations(
                &repo,
                &revision,
                &desired_state_dir,
                &run_id,
                &job_ids,
                &version,
                &alias,
                full,
                json,
            )
            .await
        }
        ArtifactCommands::List {
            type_name,
            namespace,
            name,
            label,
            json,
        } => list(&type_name, &namespace, &name, &label, json).await,
        ArtifactCommands::Show { r#ref, json } => show(&r#ref, json).await,
        ArtifactCommands::Resolve { r#ref, json } => resolve(&r#ref, json).await,
        ArtifactCommands::Publish {
            manifest_path,
            verify: _,
            no_verify,
            full,
            json,
        } => {
            // Python default is --verify; --no-verify flips it off.
            publish(&manifest_path, !no_verify, full, json).await
        }
        ArtifactCommands::Alias(ArtifactAliasCommands::Set {
            target_ref,
            alias,
            expected_previous,
            json,
        }) => alias_set(&target_ref, &alias, expected_previous.as_deref(), json).await,
        ArtifactCommands::Verify { r#ref, full, json } => verify(&r#ref, full, json).await,
        ArtifactCommands::Lineage { r#ref, json } => lineage(&r#ref, json).await,
    }
}
