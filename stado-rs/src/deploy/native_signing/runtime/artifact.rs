use super::{cache, Prepared, Receipt, PRODUCT, VERSION};
use anyhow::{ensure, Context, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub(super) async fn install(root: &Path, platform: &str) -> Result<Prepared> {
    let artifact =
        crate::cli::release_cmd::verified_artifact_for_submit(PRODUCT, VERSION, platform)
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))
            .with_context(|| {
                format!("resolve qualified native SDK {PRODUCT}/{VERSION}/{platform}")
            })?;
    crate::release_control::CoordinateRevision::new(
        PRODUCT,
        VERSION,
        platform,
        &artifact.source_revision,
    )
    .map_err(anyhow::Error::msg)?;
    let bytes = crate::cli::storage::fetch_object(&artifact.archive_uri)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    ensure!(
        crate::release_control::sha256_bytes(&bytes) == artifact.artifact_sha256,
        "native SDK archive differs from its signed digest: {}",
        artifact.archive_uri
    );
    let parent = root.parent().context("native SDK cache has no parent")?;
    let staging = tempfile::Builder::new()
        .prefix(".native-sdk-")
        .tempdir_in(parent)?;
    let payload = staging.path().join("payload");
    crate::release_control::safe_extract_archive(&bytes, &payload).map_err(anyhow::Error::msg)?;
    drop(bytes);
    let program = payload.join(PRODUCT);
    cache::native(&program, platform)?;
    let (executable_bytes, executable_sha256) =
        crate::release_control::sha256_file(&program).map_err(anyhow::Error::msg)?;
    let receipt = Receipt {
        schema_version: 1,
        version: VERSION.into(),
        platform: platform.into(),
        artifact,
        executable_bytes,
        executable_sha256,
    };
    let mut record = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(payload.join("sdk-receipt.json"))?;
    serde_json::to_writer_pretty(&mut record, &receipt)?;
    record.write_all(b"\n")?;
    record.sync_all()?;
    File::open(&program)?.sync_all()?;
    File::open(&payload)?.sync_all()?;
    ensure!(
        !root.try_exists()?,
        "another native SDK installation appeared at {}",
        root.display()
    );
    fs::rename(&payload, root)
        .with_context(|| format!("commit qualified native SDK {}", root.display()))?;
    File::open(parent)?.sync_all()?;
    Ok(Prepared {
        program: root.join(PRODUCT),
        receipt,
    })
}
