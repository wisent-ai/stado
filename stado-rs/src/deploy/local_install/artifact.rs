//! Stage one: read the release artifact and place the binaries a unit
//! ExecStarts. The manifest identity and the archive digest are verified
//! before anything is extracted, every placed binary leaves an attestation
//! receipt `stado service converge` reads, and the paths under
//! `~/.stado/bin/` are fixed so a reinstall never disturbs a live ExecStart.

use std::path::Path;

use serde_json::Value;

use crate::deploy::DeployError;

/// Executables baked into the unit ExecStart: the release binaries under
/// `~/.stado/bin/` (populated by [`ensure_bins`] on non-dry-run installs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bins {
    pub stado: String,
    pub stado_fix: String,
    pub stado_watchdog: String,
}

impl Bins {
    /// The fixed `~/.stado/bin/` paths — stable across reinstalls so the
    /// unit's ExecStart (and therefore the capacity broadcast loop) is
    /// undisturbed by re-provisioning.
    pub fn resolve(home: &Path) -> Self {
        let bin_dir = home.join(".stado").join("bin");
        let path = |name: &str| bin_dir.join(name).to_string_lossy().into_owned();
        Self {
            stado: path("stado"),
            stado_fix: path("stado-fix"),
            stado_watchdog: path("stado-watchdog"),
        }
    }
}

/// Release binaries the local services ExecStart (stado covers agent /
/// coordinator / disk-cleanup, stado-fix the failure-fixer loop,
/// stado-watchdog the diagnostics watchdog).
pub const LOCAL_BINARIES: [&str; 3] = ["stado", "stado-fix", "stado-watchdog"];

/// Release platform dir for this host (same mapping as
/// [`crate::self_update::platform_triple_short`]).
fn release_platform() -> Result<&'static str, DeployError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("linux", "x86_64") => Ok("linux-amd64"),
        (os, arch) => Err(DeployError(format!(
            "no release triple for platform {os}-{arch} (supported: linux-amd64, darwin-arm64)"
        ))),
    }
}

/// Populate `~/.stado/bin/` from the exact archive configured by
/// [`crate::config::stado_release_version`] when any service binary is missing.
/// The canonical manifest digest is verified before extraction or installation.
pub async fn ensure_bins(home: &Path, echo: &mut dyn FnMut(&str)) -> Result<(), DeployError> {
    ensure_bins_with(home, &crate::self_update::HttpReleaseFetcher::new(), echo).await
}

/// [`ensure_bins`] against an injected fetcher (offline tests).
pub async fn ensure_bins_with(
    home: &Path,
    fetcher: &impl crate::self_update::ReleaseFetcher,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let version = crate::config::stado_release_version();
    ensure_bins_at_version_with(home, &version, fetcher, echo).await
}

async fn ensure_bins_at_version_with(
    home: &Path,
    version: &str,
    fetcher: &impl crate::self_update::ReleaseFetcher,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    use std::os::unix::fs::PermissionsExt;

    use crate::self_update::sha256_hex;
    let bin_dir = home.join(".stado").join("bin");
    if LOCAL_BINARIES
        .iter()
        .all(|name| bin_dir.join(name).is_file())
    {
        return Ok(());
    }
    let platform = release_platform()?;
    std::fs::create_dir_all(&bin_dir).map_err(|exc| DeployError(exc.to_string()))?;
    if version.is_empty()
        || !version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(DeployError(
            "STADO_RELEASE_VERSION must be an exact immutable release coordinate".to_string(),
        ));
    }
    let prefix = format!("{version}/{platform}");
    let manifest_name = format!("release-manifest-{platform}.json");
    let manifest_bytes = fetcher
        .fetch(&format!("{prefix}/{manifest_name}"))
        .await
        .map_err(|exc| {
            DeployError(format!(
                "release download failed for {manifest_name}: {exc}"
            ))
        })?
        .ok_or_else(|| DeployError(format!("{manifest_name} is not published")))?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| DeployError(format!("invalid release manifest: {error}")))?;
    let object = manifest
        .as_object()
        .ok_or_else(|| DeployError("release manifest must be an object".to_string()))?;
    if object.len() != 5
        || manifest.get("product").and_then(Value::as_str) != Some("stado")
        || manifest.get("version").and_then(Value::as_str) != Some(version)
        || manifest.get("platform").and_then(Value::as_str) != Some(platform)
        || !manifest
            .get("source_commit")
            .and_then(Value::as_str)
            .is_some_and(|commit| {
                matches!(commit.len(), 40 | 64)
                    && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    {
        return Err(DeployError(
            "release manifest identity is invalid".to_string(),
        ));
    }
    let expected = manifest
        .get("sha256")
        .and_then(Value::as_str)
        .filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| DeployError("release manifest sha256 is invalid".to_string()))?;
    let archive_name = format!("stado-v{version}-{platform}.tar.gz");
    let archive = fetcher
        .fetch(&format!("{prefix}/{archive_name}"))
        .await
        .map_err(|exc| DeployError(format!("release download failed for {archive_name}: {exc}")))?
        .ok_or_else(|| DeployError(format!("{archive_name} is not published")))?;
    let actual = sha256_hex(&archive);
    if actual != expected {
        return Err(DeployError(format!(
            "sha256 mismatch for {archive_name}: expected {expected}, got {actual}"
        )));
    }
    let staging = tempfile::tempdir().map_err(|error| DeployError(error.to_string()))?;
    let extracted = staging.path().join("archive");
    crate::release_control::safe_extract_archive(&archive, &extracted).map_err(DeployError)?;
    let mut verified: Vec<(&str, Vec<u8>)> = Vec::with_capacity(LOCAL_BINARIES.len());
    for name in LOCAL_BINARIES {
        let path = extracted.join(name);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeployError(error.to_string()))?;
        if !metadata.file_type().is_file() || metadata.len() == 0 {
            return Err(DeployError(format!(
                "release archive member {name} is not a non-empty regular file"
            )));
        }
        let bytes = std::fs::read(path).map_err(|error| DeployError(error.to_string()))?;
        verified.push((name, bytes));
    }
    for (name, bytes) in verified {
        let dest = bin_dir.join(name);
        std::fs::write(&dest, &bytes).map_err(|exc| DeployError(exc.to_string()))?;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
            .map_err(|exc| DeployError(exc.to_string()))?;
        // The receipt `stado service converge` attests against. These bytes were
        // verified against the canonical manifest digest above, and this path
        // used to throw that evidence away exactly as the self-update path did
        // before it was fixed — leaving a host that had been delivered a
        // published release reading `unattested`. Never fatal: the install is
        // the point, and a receipt that cannot be written is reported.
        if let Err(error) =
            crate::self_update::stage_for_attestation(name, version, platform, &dest)
        {
            echo(&format!(
                "[install] {name} {version} installed but its attestation copy could not be \
                 staged, so `stado service converge` will read it as unattested: {error}"
            ));
        }
    }
    echo(&format!(
        "[install] downloaded stado {} ({platform}) -> {}",
        version,
        bin_dir.display()
    ));
    Ok(())
}
