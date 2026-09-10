//! The artifact the builder leaves behind: the staged tree it packages, the
//! receipt that stands for it, and the measure of the scratch it wrote.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flate2::{Compression, GzBuilder};

use crate::cli::CmdError;
use crate::release_pipeline::{
    tree_bytes, ArtifactReceipt, BuildReceipt, ReceiptInput, ScratchReceipt, StepReceipt,
    StepStatus, WorkerRequest, SCRATCH_LEAF,
};

fn collect(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    let path = root.join(relative);
    // The recipe's stage map is a declaration about what the build produces, and
    // this is where the two are compared. A bare `?` here reported only
    // `No such file or directory (os error 2)`, so a stage entry whose producer
    // had been deleted looked identical to a broken builder.
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        CmdError::click(format!(
            "staged path {} declared by the recipe is not there: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(CmdError::click(format!(
            "staged path is a symlink: {}",
            path.display()
        )));
    }
    if metadata.is_file() {
        out.push(relative.into());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(CmdError::click("staged path is not regular"));
    }
    let mut entries: Vec<_> = std::fs::read_dir(path)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        collect(root, &relative.join(entry.file_name()), out)?
    }
    Ok(())
}
pub(super) fn package(
    source: &Path,
    stage: &BTreeMap<String, String>,
) -> Result<Vec<u8>, CmdError> {
    let gz = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    let mut archive = tar::Builder::new(gz);
    for (from, to) in stage {
        let base = Path::new(from);
        let mut paths = Vec::new();
        collect(source, base, &mut paths)?;
        for path in paths {
            let suffix = path.strip_prefix(base).unwrap_or(Path::new(""));
            let destination = if suffix.as_os_str().is_empty() {
                PathBuf::from(to)
            } else {
                Path::new(to).join(suffix)
            };
            let bytes = std::fs::read(source.join(&path))?;
            let metadata = std::fs::metadata(source.join(&path))?;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                header.set_mode(metadata.permissions().mode() & 0o777);
            }
            #[cfg(not(unix))]
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, destination, bytes.as_slice())?
        }
    }
    Ok(archive.into_inner()?.finish()?)
}
pub(super) fn write_receipt(receipt: &BuildReceipt) -> Result<(), CmdError> {
    std::fs::create_dir_all("output")?;
    std::fs::write("output/receipt.json", serde_json::to_vec(receipt)?)?;
    Ok(())
}

/// Measure the scratch tree at `root` and the free bytes on its volume, now,
/// before the tree is removed. Nothing is written: a build that filled the
/// volume has left no room for the record until its tree is gone.
pub(super) fn measure_scratch(
    root: &Path,
    request: &WorkerRequest,
    job_id: &str,
    build: StepStatus,
) -> Result<ScratchReceipt, CmdError> {
    let bytes = tree_bytes(root).map_err(|error| {
        CmdError::click(format!(
            "cannot measure the build tree {}: {error}",
            root.display()
        ))
    })?;
    let stat = nix::sys::statvfs::statvfs(root).map_err(|error| {
        CmdError::click(format!(
            "cannot read the free space of the volume holding {}: {error}",
            root.display()
        ))
    })?;
    Ok(ScratchReceipt {
        schema_version: 1,
        run_id: request.run_id.clone(),
        job_id: job_id.to_owned(),
        product: request.product.clone(),
        platform: request.platform.clone(),
        builder: request.builder.clone(),
        bytes,
        free_bytes: stat.blocks_available() as u64 * stat.fragment_size() as u64,
        build,
        measured_at: chrono::Utc::now().to_rfc3339(),
    })
}

pub(super) fn write_scratch(scratch: &ScratchReceipt) -> Result<(), CmdError> {
    std::fs::create_dir_all("output")?;
    std::fs::write(
        Path::new("output").join(SCRATCH_LEAF),
        serde_json::to_vec(scratch)?,
    )?;
    Ok(())
}

/// The sentence a failed build's receipt and exit carry about the disk it
/// was writing: the two numbers that were one line inside a 30 KB log when
/// the 0.20.3 darwin build died on charless-mac-mini.
pub(super) fn disk_sentence(scratch: &ScratchReceipt) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let state = if scratch.exhausted_disk() {
        "ran out of disk"
    } else {
        "had room"
    };
    format!(
        "the build tree held {:.1} GiB and its volume had {:.1} GiB free ({state})",
        scratch.bytes as f64 / GIB,
        scratch.free_bytes as f64 / GIB
    )
}

/// The receipt every outcome of one build job writes.
///
/// A build ends in one of three places — a quality gate refused, the build
/// command refused, or an artifact exists — and each of them recorded the
/// same seventeen fields inline. Three copies of one record is three chances
/// for one of them to stop matching the other two, which is exactly the
/// record an operator reads when a release is in doubt.
#[allow(clippy::too_many_arguments)]
pub(super) fn receipt(
    request: &WorkerRequest,
    job_id: &str,
    inputs: BTreeMap<String, ReceiptInput>,
    quality: Vec<StepReceipt>,
    build: StepReceipt,
    status: StepStatus,
    artifact: Option<ArtifactReceipt>,
    failure: Option<String>,
) -> BuildReceipt {
    BuildReceipt {
        schema_version: 1,
        run_id: request.run_id.clone(),
        job_id: job_id.to_string(),
        product: request.product.clone(),
        version: request.version.clone(),
        platform: request.platform.clone(),
        builder: request.builder.clone(),
        source_commit: request.source_commit.clone(),
        source_sha256: request.source_sha256.clone(),
        manifest_sha256: request.manifest_sha256.clone(),
        inputs,
        secret_env: request.secret_env.clone(),
        quality,
        build,
        status,
        artifact,
        completed_at: chrono::Utc::now().to_rfc3339(),
        failure,
    }
}
