//! The release authority's key material: generating a pair, reading the
//! private half out of the credential store, and signing one candidate.

use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::json;

use crate::cli::CmdError;
use crate::release_control::{self, QualificationStatus, ReleaseQualification};
use crate::release_pipeline::{BuildReceipt, StepStatus};

use super::publish::{publish_pipeline_release, PipelinePublishRequest};
use super::{ReleaseKeygenArgs, ReleasePrepareArgs};

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CmdError> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", path.display())))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_public(path: &Path, bytes: &[u8]) -> Result<(), CmdError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", path.display())))?;
    writeln!(file, "{}", BASE64.encode(bytes))?;
    file.sync_all()?;
    Ok(())
}

pub(in crate::cli::release_cmd) async fn keygen(args: &ReleaseKeygenArgs) -> Result<(), CmdError> {
    if args.key_id.is_empty() {
        return Err(CmdError::usage("--key-id must not be empty"));
    }
    let (private, public) = release_control::generate_signing_key().map_err(CmdError::click)?;
    write_private(&args.private_key, &private)?;
    if let Err(error) = write_public(&args.public_key, &public) {
        let _ = std::fs::remove_file(&args.private_key);
        return Err(error);
    }
    println!(
        "generated release key {} private={} public={}",
        args.key_id,
        args.private_key.display(),
        args.public_key.display()
    );
    Ok(())
}

async fn signing_key(item: &str) -> Result<Vec<u8>, CmdError> {
    let encoded = crate::credential_store::read_string(item, "private_key")
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "Skarbiec item {item:?} field private_key is required"
            ))
        })?;
    BASE64
        .decode(encoded)
        .map_err(|_| CmdError::click("release signing key field is not base64"))
}

pub(in crate::cli::release_cmd) async fn prepare(
    args: &ReleasePrepareArgs,
) -> Result<(), CmdError> {
    let archive = std::fs::read(&args.archive)?;
    let qualification_receipt = std::fs::read(&args.qualification)?;
    let receipt: BuildReceipt = serde_json::from_slice(&qualification_receipt)?;
    let artifact_sha256 = release_control::sha256_bytes(&archive);
    if receipt.product != args.product
        || receipt.version != args.version
        || receipt.platform != args.platform
        || receipt.builder != args.builder
        || receipt.source_commit != args.source_revision
        || receipt.source_sha256 != args.source_sha256
        || receipt.manifest_sha256 != args.pipeline_manifest_sha256
        || receipt.status != StepStatus::Passed
        || receipt.artifact.as_ref().map(|value| value.sha256.as_str())
            != Some(artifact_sha256.as_str())
    {
        return Err(CmdError::click(
            "qualification receipt does not describe this prepared artifact",
        ));
    }
    let qualification = ReleaseQualification {
        status: QualificationStatus::Passed,
        evidence_sha256: Some(release_control::sha256_bytes(&qualification_receipt)),
        completed_at: Some(receipt.completed_at),
    };
    let private = match (&args.signing_key_item, &args.signing_key_file) {
        (Some(item), None) => signing_key(item).await?,
        (None, Some(path)) => {
            use std::os::unix::fs::PermissionsExt as _;
            let metadata = std::fs::metadata(path)?;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(CmdError::click(
                    "release signing key file must be owner-only",
                ));
            }
            std::fs::read(path)?
        }
        _ => {
            return Err(CmdError::usage(
                "prepare needs exactly one of --signing-key-item or --signing-key-file",
            ))
        }
    };
    let public = release_control::signing_public_key(&private).map_err(CmdError::click)?;
    let (artifact, manifest) = publish_pipeline_release(PipelinePublishRequest {
        product: &args.product,
        version: &args.version,
        platform: &args.platform,
        archive: &archive,
        source_revision: &args.source_revision,
        source_sha256: &args.source_sha256,
        pipeline_manifest_sha256: &args.pipeline_manifest_sha256,
        binary: &args.binary,
        launcher: &args.launcher,
        config_schema: args.config_schema,
        state_schema: args.state_schema,
        minimum_stado_version: &args.minimum_stado_version,
        rollback_compatible_with: &args.rollback_compatible_with,
        qualification,
        qualification_receipt: &qualification_receipt,
        key_id: &args.key_id,
        private_key: &private,
        builder: &args.builder,
    })
    .await?;
    let report = json!({
        "product": args.product,
        "version": args.version,
        "platform": args.platform,
        "source_revision": args.source_revision,
        "source_sha256": args.source_sha256,
        "pipeline_manifest_sha256": args.pipeline_manifest_sha256,
        "artifact_sha256": artifact.artifact_sha256,
        "manifest_sha256": artifact.manifest_sha256,
        "key_id": args.key_id,
        "public_key": BASE64.encode(public),
        "qualification": manifest.qualification.status,
        "archive_uri": artifact.archive_uri,
        "signature_uri": artifact.signature_uri,
        "manifest_uri": artifact.manifest_uri,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "prepared {} {} {} artifact={} manifest={} key={}",
            args.product,
            args.version,
            args.platform,
            report["artifact_sha256"].as_str().unwrap_or_default(),
            report["manifest_sha256"].as_str().unwrap_or_default(),
            args.key_id
        );
    }
    Ok(())
}
