//! One declared quality gate of a committed revision, run on a fleet host
//! exactly as the revision's release manifest declares its argv: the same
//! committed-source export, delivery, toolchain prelude, shared Cargo target
//! and removal as a Cargo operation ([`super::run`]), with the export kept
//! in the checkout's build area under Stado's home, the history records a
//! release build's archive carries beside the commit's files written into
//! it, and the gate's argv run in the delivered tree's root in place of the
//! Cargo command. Its caller, the submit queue, reads the argv from the
//! manifest the revision commits, the one a release worker runs, never from
//! text a person typed.

use std::path::Path;

use crate::deploy::{DeployError, Runner};
use crate::targets::ComputeTarget;

use super::{execute, refused, CargoOperation, HostCargoRequest};

/// Everything one gate run did.
#[derive(Debug)]
pub struct HostGateReceipt {
    pub target: String,
    pub commit: String,
    pub archive_sha256: String,
    pub argv: Vec<String>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub run_directory: String,
}

/// Refuse `argv`, a quality gate `manifest` declares, before anything is
/// exported: the refusal every host would answer alike.
pub fn validate(manifest: &Path, argv: &[String]) -> Result<(), DeployError> {
    if argv.is_empty() {
        return Err(refused(format!(
            "a gate of {} declares an empty argv, so there is nothing to run",
            manifest.display()
        )));
    }
    super::super::validate_arguments(argv).map_err(refused)
}

/// Run `argv`, a quality gate `manifest` (the checkout's release manifest)
/// declares, over `revision` on `target`, with the release worker's
/// contract for `version` and `platform` and `records` (each a path in the
/// source and its bytes: the verified baseline provenance a release build's
/// archive carries) in the delivered tree; the gate's own failure is the
/// receipt's `exit_code`.
pub async fn run(
    target: &ComputeTarget,
    manifest: &Path,
    revision: &str,
    (argv, version, platform): (&[String], &str, &str),
    records: &[(&str, Vec<u8>)],
    runner: &Runner,
) -> Result<HostGateReceipt, DeployError> {
    validate(manifest, argv)?;
    let request = HostCargoRequest {
        manifest_path: manifest.to_path_buf(),
        revision: revision.to_string(),
        // No Cargo command runs; any operation but `Build` also keeps
        // [`execute`] from bringing binaries back.
        operation: CargoOperation::Check,
        binaries: Vec::new(),
        arguments: Vec::new(),
    };
    let gate = Some((argv, version, platform));
    let ran = execute(target, &request, gate, records, runner).await?;
    Ok(HostGateReceipt {
        target: ran.target,
        commit: ran.commit,
        archive_sha256: ran.archive_sha256,
        argv: argv.to_vec(),
        exit_code: ran.exit_code,
        stdout: ran.stdout,
        stderr: ran.stderr,
        run_directory: ran.run_directory,
    })
}
