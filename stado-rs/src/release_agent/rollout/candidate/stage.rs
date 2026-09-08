//! Install one verified archive into its immutable directory, and choose the
//! port the next candidate starts on.

use std::path::{Path, PathBuf};

use crate::release_agent::state::document::atomic_json;
use crate::release_agent::state::records::HostReleaseState;
use crate::release_control::{self, ReleaseManifest};

pub(crate) fn marker_path(directory: &Path) -> PathBuf {
    directory.join(".stado-release.json")
}

pub(crate) fn stage_release(
    manifest: &ReleaseManifest,
    archive: &[u8],
    directory: &Path,
) -> Result<(), String> {
    let manifest_sha =
        release_control::sha256_bytes(&release_control::canonical_manifest(manifest)?);
    if directory.exists() {
        let marker = std::fs::read(marker_path(directory)).map_err(|_| {
            format!(
                "immutable release directory has no marker: {}",
                directory.display()
            )
        })?;
        let installed: ReleaseManifest = serde_json::from_slice(&marker).map_err(|_| {
            format!(
                "immutable release marker is invalid: {}",
                directory.display()
            )
        })?;
        if release_control::sha256_bytes(&release_control::canonical_manifest(&installed)?)
            != manifest_sha
        {
            return Err(format!(
                "immutable release directory contains a different manifest: {}",
                directory.display()
            ));
        }
        return Ok(());
    }
    release_control::safe_extract_archive(archive, directory)?;
    atomic_json(&marker_path(directory), manifest)
}

pub(crate) fn next_port(
    candidate_ports: [u16; 2],
    state: &HostReleaseState,
    occupied: Option<u16>,
) -> u16 {
    // With no active record the previous rule always chose the first candidate
    // port -- exactly where a de-facto active from a lost rollout is still
    // serving, so the new candidate died on the bind and the rollout could never
    // proceed. The proxy's upstream is authoritative when the record is silent.
    match state.active.as_ref().map(|active| active.port).or(occupied) {
        Some(port) if port == candidate_ports[0] => candidate_ports[1],
        _ => candidate_ports[0],
    }
}
