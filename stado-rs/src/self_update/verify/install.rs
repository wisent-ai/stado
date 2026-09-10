//! The download and its verification: fetch the configured release, refuse
//! anything whose identity or digest does not match the published manifest,
//! and only then hand the verified files to the swap.

use std::path::{Path, PathBuf};

use crate::binary::release::version_newer;
use crate::self_update::swap::replace::replace_verified;
use crate::self_update::{
    platform_triple_short, recycle_replaced_units, sha256_hex, stage_for_attestation,
    update_targets, HttpReleaseFetcher, ReleaseFetcher, SelfUpdateError, UpdateOutcome,
};

use super::targets::current_exe_path;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseArchiveManifest {
    product: String,
    version: String,
    platform: String,
    source_commit: String,
    sha256: String,
}

/// Install the configured exact release when it is newer than this binary.
pub async fn self_update(log_fn: &mut dyn FnMut(&str)) -> Result<UpdateOutcome, SelfUpdateError> {
    let fetcher = HttpReleaseFetcher::new();
    if let Some(error) = &fetcher.configuration_error {
        return Err(SelfUpdateError::Fetch(error.clone()));
    }
    let current_exe = current_exe_path()?;
    let host_platform = platform_triple_short()?;
    if fetcher.platform != host_platform {
        return Err(SelfUpdateError::Fetch(format!(
            "configured release platform {:?} does not match this host {:?}",
            fetcher.platform, host_platform
        )));
    }
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let to = fetcher.version.clone();
    if !version_newer(&installed, &to) {
        return Ok(UpdateOutcome::UpToDate {
            installed,
            latest: to,
        });
    }
    install_release_with(&fetcher, installed, to, host_platform, &current_exe, log_fn).await
}

async fn install_release_with(
    fetcher: &impl ReleaseFetcher,
    installed: String,
    to: String,
    platform: &str,
    current_exe: &Path,
    log_fn: &mut dyn FnMut(&str),
) -> Result<UpdateOutcome, SelfUpdateError> {
    let install_dir = current_exe
        .parent()
        .ok_or_else(|| SelfUpdateError::NoInstallDir(current_exe.to_path_buf()))?;
    let targets = update_targets(install_dir, current_exe)?;
    let staging = tempfile::tempdir_in(install_dir).map_err(|error| {
        SelfUpdateError::InstallDirNotWritable(install_dir.to_path_buf(), error.to_string())
    })?;
    let prefix = format!("{to}/{platform}");
    let manifest_name = format!("release-manifest-{platform}.json");
    let manifest_bytes = fetcher
        .fetch(&format!("{prefix}/{manifest_name}"))
        .await?
        .ok_or_else(|| SelfUpdateError::Fetch(format!("{prefix}/{manifest_name} is missing")))?;
    let manifest: ReleaseArchiveManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| SelfUpdateError::Fetch(format!("invalid release manifest: {error}")))?;
    if manifest.product != "stado"
        || manifest.version != to
        || manifest.platform != platform
        || !matches!(manifest.source_commit.len(), 40 | 64)
        || !manifest
            .source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || manifest.sha256.len() != 64
        || !manifest
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SelfUpdateError::Fetch(
            "release manifest identity or digest is invalid".to_string(),
        ));
    }
    let archive_name = format!("stado-v{to}-{platform}.tar.gz");
    let archive_bytes = fetcher
        .fetch(&format!("{prefix}/{archive_name}"))
        .await?
        .ok_or_else(|| SelfUpdateError::Fetch(format!("{prefix}/{archive_name} is missing")))?;
    let actual = sha256_hex(&archive_bytes);
    if actual != manifest.sha256 {
        return Err(SelfUpdateError::HashMismatch {
            name: archive_name,
            expected: manifest.sha256,
            actual,
        });
    }
    let extracted = staging.path().join("archive");
    crate::release_control::safe_extract_archive(&archive_bytes, &extracted)
        .map_err(SelfUpdateError::Fetch)?;
    let mut staged: Vec<(String, PathBuf)> = Vec::with_capacity(targets.len());
    for name in &targets {
        let staged_path = extracted.join(name);
        let metadata = std::fs::symlink_metadata(&staged_path)?;
        if !metadata.file_type().is_file() || metadata.len() == 0 {
            return Err(SelfUpdateError::Fetch(format!(
                "release archive member {name} is not a non-empty regular file"
            )));
        }
        log_fn(&format!("self-update: verified {name} {to}"));
        staged.push((name.clone(), staged_path));
    }
    // Leave the receipt the fleet's provenance check reads.
    //
    // `stado host release` stages every binary it delivers at
    // `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, and
    // `cli::service_converge::attest_installed` decides provenance by
    // comparing the installed file against exactly that path. Self-update is
    // the other delivery path and it staged nothing: it verified these bytes
    // against the published SHA-256 manifest a few lines above, installed
    // them, and threw the evidence away. So every binary self-update ever
    // delivered read `unattested` afterwards — the fleet had the provenance
    // and discarded it, then reported the result as if the bytes were
    // untrustworthy.
    //
    // On 2026-09-01 `lukasz-macbook` reported exactly that for `stado`: nine
    // versions staged by `host release`, the newest 0.13.24, and an installed
    // binary with no staged copy at all.
    //
    // Never fatal. The bytes are verified and the install is the point; a
    // receipt that cannot be written is logged and the update continues.
    for (name, staged_path) in &staged {
        if let Err(error) = stage_for_attestation(name, &to, platform, staged_path) {
            log_fn(&format!(
                "self-update: {name} {to} installed but its attestation copy could not be \
                 staged, so `stado service converge` will read it as unattested: {error}"
            ));
        }
    }
    for (name, staged_path) in &staged {
        replace_verified(staged_path, &install_dir.join(name))?;
        log_fn(&format!("self-update: installed {name} {to}"));
    }
    std::fs::File::open(install_dir)?.sync_all()?;
    if let Err(error) = recycle_replaced_units("self-update", install_dir, &targets, log_fn).await {
        log_fn(&format!("self-update: {error}"));
    }
    Ok(UpdateOutcome::Updated {
        from: installed,
        to,
    })
}
