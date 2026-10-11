//! The immutable objects a submission writes before anything is built, and
//! where every run and build keeps its objects. What the committed tree is
//! made of is read in `checkout`.

use std::collections::BTreeMap;

use crate::cli::storage;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::PipelineChannel;

pub(crate) async fn immutable(
    uri: &str,
    bytes: &[u8],
    kind: &str,
    meta: &BTreeMap<String, String>,
) -> Result<(), CmdError> {
    match storage::fetch_object_from_writer(uri).await {
        Ok(v) if v == bytes => return Ok(()),
        Ok(_) => {
            return Err(CmdError::refused(format!(
                "immutable object differs: {uri}"
            )))
        }
        Err(_) => {}
    }
    let f = tempfile::NamedTempFile::new()?;
    std::fs::write(f.path(), bytes)?;
    storage::store_object_with_metadata(uri, &f.path().display().to_string(), kind, true, meta)
        .await?;
    Ok(())
}
pub(crate) fn run_path(product: &str, id: &str, leaf: &str) -> String {
    format!("runs/release-pipeline/{product}/{id}/{leaf}")
}
pub(crate) fn run_uri(product: &str, id: &str, leaf: &str) -> String {
    format!(
        "stado://{}/{}",
        crate::config::wc_stado_storage_namespace(),
        run_path(product, id, leaf)
    )
}
pub(crate) fn run_state_path(id: &str) -> String {
    format!("runs/release-pipeline/{id}/run.json")
}
/// Where the index of unfinished runs keeps one empty marker per run: the
/// runs a finishing pass may still have to move. [`super::state::save`]
/// writes it while the run is unfinished and deletes it when it finishes.
pub(crate) const UNFINISHED_RUN_PREFIX: &str = "runs/release-pipeline-unfinished/";
pub(crate) fn unfinished_run_path(id: &str) -> String {
    format!("{UNFINISHED_RUN_PREFIX}{id}")
}
/// Where a build keeps what its jobs consume: the source archive, the
/// manifest, every staged input and each platform's immutable request.
/// A release run of the same source reads them from here; it stages nothing
/// of its own.
pub(crate) fn build_path(product: &str, build_id: &str, leaf: &str) -> String {
    format!("runs/build/{product}/{build_id}/{leaf}")
}
pub(crate) fn build_uri(product: &str, build_id: &str, leaf: &str) -> String {
    format!(
        "stado://{}/{}",
        crate::config::wc_stado_storage_namespace(),
        build_path(product, build_id, leaf)
    )
}
pub(crate) fn build_state_path(build_id: &str) -> String {
    format!("runs/build/{build_id}/run.json")
}
/// Where a run's staged source archive is: under the build it consumes, or
/// under the run itself for a run recorded before builds had records of
/// their own, whose deliveries still read what that run staged.
pub(crate) fn run_source_input_uri(run: &crate::release_pipeline::ReleaseRun) -> String {
    match run.build_id.as_deref() {
        Some(build_id) => build_uri(&run.product, build_id, "inputs/source.tar.gz"),
        None => run_uri(&run.product, &run.run_id, "inputs/source.tar.gz"),
    }
}
/// Where a run's manifest copy is, by the same rule.
pub(crate) fn run_manifest_path(run: &crate::release_pipeline::ReleaseRun) -> String {
    match run.build_id.as_deref() {
        Some(build_id) => build_path(&run.product, build_id, "manifest.json"),
        None => run_path(&run.product, &run.run_id, "manifest.json"),
    }
}
pub(crate) async fn queue_immutable(path: &str, bytes: &[u8]) -> Result<(), CmdError> {
    let store = JobStorage::new().await.map_err(CmdError::from)?;
    if let Some(existing) = store.read_bytes(path).await.map_err(CmdError::from)? {
        return if existing == bytes {
            Ok(())
        } else {
            Err(CmdError::refused(format!(
                "immutable queue object differs: {path}"
            )))
        };
    }
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), bytes)?;
    if store
        .upload_file_if_absent(path, file.path())
        .await
        .map_err(CmdError::from)?
    {
        return Ok(());
    }
    match store.read_bytes(path).await.map_err(CmdError::from)? {
        Some(existing) if existing == bytes => Ok(()),
        _ => Err(CmdError::refused(format!(
            "immutable queue object raced with different bytes: {path}"
        ))),
    }
}
pub(crate) fn identity(
    product: &str,
    version: &str,
    channel: PipelineChannel,
    source: &str,
    manifest: &str,
) -> String {
    release_control::sha256_bytes(
        format!("{product}\0{version}\0{channel:?}\0{source}\0{manifest}").as_bytes(),
    )[..32]
        .into()
}
/// A build's identity has no channel in it: the channel is the release's
/// question, and one build answers it for every channel.
pub(crate) fn build_identity(product: &str, version: &str, source: &str, manifest: &str) -> String {
    release_control::sha256_bytes(
        format!("build\0{product}\0{version}\0{source}\0{manifest}").as_bytes(),
    )[..32]
        .into()
}
