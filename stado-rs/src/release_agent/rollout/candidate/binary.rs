//! The executable of the release this host currently records and routes.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::stage::marker_path;
use crate::release_agent::rollout::processes::inventory::release_processes;
use crate::release_agent::rollout::serving::discover::proxy_process_matches;
use crate::release_agent::rollout::serving::proxy::ProxyState;
use crate::release_agent::state::document::{load_state, proxy_state_path};
use crate::release_agent::state::records::ActiveBinary;
use crate::release_control::{
    self, ProductReleasePolicy, QualificationStatus, ReleaseManifest, ReleaseTargetPolicy,
};

/// Resolve the executable from the exact release the local agent currently
/// records and routes as active. Desired state is deliberately irrelevant:
/// a rejected newer candidate may be quarantined while its healthy predecessor
/// remains the release actually serving the stable bind.
pub(crate) fn active_binary(
    product: &str,
    target_name: &str,
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
) -> Result<ActiveBinary, String> {
    let state = load_state(target, product, target_name)?;
    let active = state.active.as_ref().ok_or_else(|| {
        format!(
            "{product} is release-controlled on {target_name} but has no observed active release (phase {:?})",
            state.phase
        )
    })?;
    let directory = PathBuf::from(&active.release_dir);
    let install_root = release_control::install_root_path(policy, target);
    let install_root = install_root
        .to_str()
        .ok_or_else(|| format!("{product} install root is not valid UTF-8"))?;
    // `spawn_release` starts a new process group whose leader is the `sudo`
    // monitor recorded in state. On macOS sudo keeps that monitor alive and
    // runs the release binary as its child, so the serving process has a
    // different pid but the same pgid. Bind the exact release executable,
    // version, directory and port to that recorded group; accepting only the
    // leader makes every healthy sudo-launched release unverifiable.
    let active_process_matches = release_processes(install_root).into_iter().any(|process| {
        process.process_group == active.pid
            && process.version == active.version
            && process.port == Some(active.port)
            && process.release_dir == directory
    });
    if !active_process_matches {
        return Err(format!(
            "{product} observed active process tuple pid={} version={} port={} release_dir={} does not match a live release process",
            active.pid,
            active.version,
            active.port,
            directory.display()
        ));
    }
    if state.quarantined.contains_key(&active.artifact_sha256) {
        return Err(format!(
            "{product} observed active digest {} is quarantined",
            active.artifact_sha256
        ));
    }

    let serving = target.blue_green_serving()?;
    let proxy_pid = state
        .proxy_pid
        .ok_or_else(|| format!("{product} observed active release has no recorded stable proxy"))?;
    if !proxy_process_matches(proxy_pid, target, &serving, product)? {
        return Err(format!(
            "{product} recorded stable proxy pid {proxy_pid} does not match the exact executable and arguments"
        ));
    }
    let proxy_path = proxy_state_path(target, product);
    let proxy: ProxyState =
        serde_json::from_slice(&std::fs::read(&proxy_path).map_err(|error| {
            format!("cannot read proxy target {}: {error}", proxy_path.display())
        })?)
        .map_err(|error| format!("invalid proxy target {}: {error}", proxy_path.display()))?;
    let expected_upstream = format!("127.0.0.1:{}", active.port);
    if proxy.generation != state.rollout_generation || proxy.upstream != expected_upstream {
        return Err(format!(
            "{product} stable proxy targets generation {} upstream {}, not observed active generation {} upstream {expected_upstream}",
            proxy.generation, proxy.upstream, state.rollout_generation
        ));
    }

    let marker_path = marker_path(&directory);
    let marker = std::fs::read(&marker_path).map_err(|error| {
        format!(
            "cannot read active release marker {}: {error}",
            marker_path.display()
        )
    })?;
    let manifest: ReleaseManifest = serde_json::from_slice(&marker)
        .map_err(|error| format!("active release marker is invalid: {error}"))?;
    release_control::validate_manifest(&manifest)?;
    let manifest_sha =
        release_control::sha256_bytes(&release_control::canonical_manifest(&manifest)?);
    if manifest_sha != active.manifest_sha256
        || manifest.product != product
        || manifest.version != active.version
        || manifest.platform != target.platform
        || manifest.artifact_sha256 != active.artifact_sha256
        || manifest.binary != policy.binary
        || manifest.qualification.status != QualificationStatus::Passed
    {
        return Err(format!(
            "{product} observed active release marker does not match its process identity"
        ));
    }
    let expected_directory = release_control::install_directory(policy, target, &manifest);
    if directory != expected_directory {
        return Err(format!(
            "{product} active release directory {} is not the policy-derived directory {}",
            directory.display(),
            expected_directory.display()
        ));
    }
    let path = directory.join(&policy.binary);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot inspect active binary {}: {error}", path.display()))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(format!(
            "active binary is not an executable regular file: {}",
            path.display()
        ));
    }
    Ok(ActiveBinary {
        path,
        version: manifest.version,
        platform: manifest.platform,
        artifact_sha256: manifest.artifact_sha256,
        manifest_sha256: manifest_sha,
    })
}
