//! Materialize one qualified, signed source coordinate without building or installing it.
use crate::{
    cli::CmdError,
    release_control::{self, CoordinateRevision, ReleaseArtifactRef},
};
use clap::Args;
use serde::Serialize;
use std::{fs, io::Write, path::PathBuf};

#[derive(Args)]
pub struct ReleaseFetchArgs {
    pub product: String,
    pub version: String,
    #[arg(long)]
    pub platform: String,
    /// The accepted full source commit; a different published source is refused.
    #[arg(long)]
    pub source_commit: String,
    /// An absolute archive path. Existing different bytes are never overwritten.
    #[arg(long)]
    pub destination: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize)]
struct Receipt {
    coordinate: CoordinateRevision,
    artifact: ReleaseArtifactRef,
    destination: PathBuf,
    artifact_bytes: u64,
}

pub(super) async fn fetch(args: &ReleaseFetchArgs) -> Result<(), CmdError> {
    let coordinate = CoordinateRevision::new(
        &args.product,
        &args.version,
        &args.platform,
        &args.source_commit,
    )
    .map_err(CmdError::click)?;
    if !args.destination.is_absolute() || args.destination.file_name().is_none() {
        return Err(CmdError::usage(
            "release fetch destination must be an absolute archive filename",
        ));
    }
    let artifact =
        super::verified_artifact_for_submit(&args.product, &args.version, &args.platform).await?;
    if artifact.source_revision != args.source_commit {
        return Err(CmdError::click(format!("published release attests {}, not accepted source {}; nothing was downloaded or installed", artifact.source_revision, args.source_commit)));
    }
    let artifact_bytes = match fs::symlink_metadata(&args.destination) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(CmdError::click(
                    "release fetch refuses an existing non-regular destination",
                ));
            }
            let (size, digest) =
                release_control::sha256_file(&args.destination).map_err(CmdError::click)?;
            if digest != artifact.artifact_sha256 {
                return Err(CmdError::click(format!(
                    "{} contains different bytes; nothing was overwritten",
                    args.destination.display()
                )));
            }
            size
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let bytes = crate::cli::storage::fetch_object(&artifact.archive_uri).await?;
            if release_control::sha256_bytes(&bytes) != artifact.artifact_sha256 {
                return Err(CmdError::click("downloaded archive differs from its signed digest; destination was not written"));
            }
            let size = bytes.len() as u64;
            let parent = args
                .destination
                .parent()
                .ok_or_else(|| CmdError::usage("archive destination has no parent"))?;
            fs::create_dir_all(parent).map_err(|error| {
                CmdError::click(format!(
                    "cannot create release destination parent {}: {error}",
                    parent.display()
                ))
            })?;
            let mut pending = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
                CmdError::click(format!(
                    "cannot stage verified release beside {}: {error}",
                    args.destination.display()
                ))
            })?;
            pending.write_all(&bytes).map_err(|error| {
                CmdError::click(format!("cannot write verified release: {error}"))
            })?;
            pending.as_file().sync_all().map_err(|error| {
                CmdError::click(format!("cannot persist verified release: {error}"))
            })?;
            drop(bytes);
            if let Err(error) = pending.persist_noclobber(&args.destination) {
                return Err(CmdError::click(format!("cannot publish verified archive at {} without replacing existing data: {error}", args.destination.display())));
            }
            fs::File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|error| {
                    CmdError::click(format!(
                        "cannot persist release destination directory: {error}"
                    ))
                })?;
            size
        }
        Err(error) => {
            return Err(CmdError::click(format!(
                "cannot inspect release destination {}: {error}",
                args.destination.display()
            )))
        }
    };
    let receipt = Receipt {
        coordinate,
        artifact,
        destination: args.destination.clone(),
        artifact_bytes,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!(
            "verified {}/{}/{} source={} archive={} sha256={} bytes={}",
            args.product,
            args.version,
            args.platform,
            args.source_commit,
            args.destination.display(),
            receipt.artifact.artifact_sha256,
            artifact_bytes
        );
    }
    Ok(())
}
