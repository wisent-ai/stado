//! One Cargo operation on one committed revision, run on a fleet host.
//!
//! The revision is exported from its canonical checkout (another session's
//! uncommitted edit is never read), delivered into a fresh managed run
//! directory on the host, compiled there with the revision's own toolchain,
//! the declared compiler cache and the host's shared Cargo target, and the
//! run directory is removed again. A build's binaries are pulled back into
//! the checkout's Cargo target before the removal. Everything one run did is
//! one receipt.

pub(super) mod gate;
mod script;
mod source;
mod volume;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::deploy::{host_channel, host_delivery, DeployError, Runner};
use crate::primitives::failure::FailureCode;
use crate::targets::ComputeTarget;

use super::{remove_run_directory, validate_arguments, validate_binary_name, RUN_AREA};
use source::{artifact, compiled, own_package, source};
pub use source::{Artifact, HostCargoReceipt};

/// The Cargo subcommands a host runs for a revision: the ones a quality gate
/// and a release need, each always `--locked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CargoOperation {
    /// `cargo build --locked --release`, its binaries pulled back.
    Build,
    Check,
    Clippy,
    Test,
}

impl CargoOperation {
    pub fn word(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Check => "check",
            Self::Clippy => "clippy",
            Self::Test => "test",
        }
    }
}

/// What one run is asked for. `manifest_path` is a `Cargo.toml` inside a
/// canonical checkout on this machine; `revision` names a commit that
/// checkout holds.
pub struct HostCargoRequest {
    pub manifest_path: PathBuf,
    pub revision: String,
    pub operation: CargoOperation,
    pub binaries: Vec<String>,
    pub arguments: Vec<String>,
}

/// Arguments that would take source or output selection away from this
/// command: it names the manifest, the lockfile and the target itself.
const OWNED_ARGUMENTS: &[&str] = &[
    "--manifest-path",
    "-m",
    "--config",
    "--lockfile-path",
    "--target-dir",
];

fn refused(message: String) -> DeployError {
    DeployError(message).stating(FailureCode::Refused)
}

/// Refuse what is wrong with the request before anything is exported.
pub fn validate(request: &HostCargoRequest) -> Result<(), DeployError> {
    if request.operation == CargoOperation::Build && request.binaries.is_empty() {
        return Err(refused(
            "a host build names the binaries it brings back; add --bin NAME for each".to_string(),
        ));
    }
    for binary in &request.binaries {
        validate_binary_name(binary).map_err(refused)?;
    }
    validate_arguments(&request.arguments).map_err(refused)?;
    for argument in request
        .arguments
        .iter()
        .take_while(|argument| *argument != "--")
    {
        let option = argument.split('=').next().unwrap_or(argument);
        if OWNED_ARGUMENTS.contains(&option) {
            return Err(refused(format!(
                "stado host cargo selects the manifest, lockfile and target directory itself; \
                 remove {argument}"
            )));
        }
    }
    if request
        .manifest_path
        .file_name()
        .and_then(|name| name.to_str())
        != Some("Cargo.toml")
    {
        return Err(refused(format!(
            "{} is not a Cargo.toml; name the manifest of the crate to run",
            request.manifest_path.display()
        )));
    }
    Ok(())
}

/// Run `request` on `target` and remove its run directory, whatever Cargo
/// answered. Refusals before Cargo starts are errors; Cargo's own failure is
/// the receipt's `exit_code`.
pub async fn run(
    target: &ComputeTarget,
    request: &HostCargoRequest,
    runner: &Runner,
) -> Result<HostCargoReceipt, DeployError> {
    validate(request)?;
    execute(target, request, None, runner).await
}

/// [`run`]'s export, delivery, script and removal. With `gate`, the script
/// runs that declared quality gate's argv in the delivered tree's root in
/// place of the Cargo operation ([`gate::run`]).
async fn execute(
    target: &ComputeTarget,
    request: &HostCargoRequest,
    gate: Option<(&[String], &str, &str)>,
    runner: &Runner,
) -> Result<HostCargoReceipt, DeployError> {
    let (checkout, commit, manifest) = source(&request.manifest_path, &request.revision).await?;
    let before = volume::admit(target, runner).await?;
    let name = checkout
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| refused(format!("{} has no directory name", checkout.display())))?;
    let run_id = uuid::Uuid::new_v4().to_string();
    // Where the export lives until it is delivered. A Cargo operation's is
    // the Cargo target of the manifest's own crate, which every Cargo
    // checkout ignores and where the binaries a build brings back stay. A
    // gate's manifest is the release manifest, beside which no ignored
    // target need exist, so its export goes to the checkout's build area
    // under Stado's home, as a quality check's does, never in the checkout.
    let area = match gate {
        None => request
            .manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("target")
            .join("host-cargo"),
        Some(_) => stado_product::common::runs::checkout_area(
            &crate::config_file::expand_tilde("~"),
            &checkout,
        )
        .map_err(|error| DeployError(format!("{error:#}")).stating(FailureCode::Config))?
        .join("host-gate"),
    };
    let export = area.join(format!("export-{run_id}"));
    let archive_sha256 = stado_product::export_committed_source(&checkout, &commit, &export)
        .map_err(|error| DeployError(format!("{error:#}")).stating(FailureCode::Config))?;
    // The package the run builds, read from the export the host gets: a
    // Cargo run that passes without compiling it judged another run's output.
    let package = gate
        .is_none()
        .then(|| own_package(&export.join(&manifest), &request.arguments))
        .flatten();
    let destination = format!("{RUN_AREA}/{run_id}/{name}");
    let delivered = host_delivery::deliver_host(
        &target.name,
        &export.to_string_lossy(),
        &destination,
        None,
        runner,
    )
    .await;
    std::fs::remove_dir_all(&export).map_err(DeployError::io(format!(
        "cannot remove the export {}",
        export.display()
    )))?;
    delivered?;
    let home = host_channel::remote_home(target, runner).await?;
    let run_directory = format!("{home}/{RUN_AREA}/{run_id}");
    let output = format!("{run_directory}/out");
    let manifest_path = format!("{home}/{destination}/{manifest}");
    let source_contract = (commit.as_str(), archive_sha256.as_str());
    let script = match gate {
        Some(declared) => script::gate(target, &manifest_path, &output, source_contract, declared)?,
        None => script::script(
            target,
            &manifest_path,
            &output,
            source_contract,
            request.operation,
            &request.binaries,
            &request.arguments,
        )?,
    };
    let ran = host_channel::run_script(target, &script, runner).await;
    let mut artifacts = Vec::new();
    let pulled = match &ran {
        Ok(result) if result.succeeded() && request.operation == CargoOperation::Build => {
            let local = area.join(&commit);
            let synced = host_delivery::sync_directory(
                target,
                &output,
                &local,
                host_delivery::Direction::Pull,
                runner,
            )
            .await;
            synced.and_then(|()| {
                for binary in &request.binaries {
                    artifacts.push(artifact(&local, binary)?);
                }
                Ok(())
            })
        }
        _ => Ok(()),
    };
    let removed = remove_run_directory(target, &run_directory, runner).await;
    let ran = ran?;
    pulled?;
    removed?;
    if let Some(package) = package.filter(|_| ran.succeeded()) {
        if !compiled(&ran.stderr, &package) {
            return Err(refused(format!(
                "{}: cargo {} of {commit} exited 0 without compiling or checking {package}, \
                 so its verdict is another run's output; run it again",
                target.name,
                request.operation.word()
            )));
        }
    }
    let (free_bytes_after, free_bytes_after_error) = match volume::measure(target, runner).await {
        Ok(after) => (Some(after.free_bytes), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(HostCargoReceipt {
        schema: "stado.host-cargo-receipt.v1",
        target: target.name.clone(),
        checkout,
        commit,
        archive_sha256,
        manifest,
        operation: request.operation,
        binaries: request.binaries.clone(),
        arguments: request.arguments.clone(),
        exit_code: ran.code,
        stdout: ran.stdout,
        stderr: ran.stderr,
        artifacts,
        free_bytes_before: Some(before.free_bytes),
        free_bytes_after,
        free_bytes_after_error,
        run_directory,
    })
}
