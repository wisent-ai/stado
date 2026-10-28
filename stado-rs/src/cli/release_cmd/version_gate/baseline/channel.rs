//! What the baseline generator asks the release channel, through the Stado
//! binary it was handed: a coordinate's state, the signed manifest, and the
//! archive that manifest attests, verified by digest and by byte count.

use std::path::Path;
use std::process::{Command, Output};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::primitives::failure::retry_exit_code;

/// Why the baseline could not be generated. `Unavailable` is the channel
/// saying it cannot answer now — the Stado CLI's retryable exit status — and
/// is reported with that same status, so the caller never reads an outage
/// as a verdict on the revision under judgement.
pub(in crate::cli::release_cmd::version_gate) enum Refusal {
    Unavailable(String),
    Invalid(String),
}

impl From<String> for Refusal {
    fn from(detail: String) -> Self {
        Self::Invalid(detail)
    }
}

pub(super) fn release_base(version: &str, platform: &str) -> String {
    format!("stado://releases/stado/{version}/{platform}")
}

/// The CLI's own stderr, or `when_silent` when it said nothing; classed by
/// the exit status the fleet reserves for retryable failures.
fn refusal(output: &Output, when_silent: String) -> Refusal {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if detail.is_empty() {
        when_silent
    } else {
        detail
    };
    if output.status.code() == Some(retry_exit_code()) {
        Refusal::Unavailable(detail)
    } else {
        Refusal::Invalid(detail)
    }
}

fn storage(stado: &Path, args: &[&str]) -> Result<Output, Refusal> {
    crate::wait::output(Command::new(stado).arg("storage").args(args))
        .map_err(|error| Refusal::Invalid(format!("{}: {error}", stado.display())))
}

/// One object's state as `storage stat --json` names it. Any other state
/// (refused, unavailable, or a word this build does not know) fails to
/// decode and is refused with the channel's own answer.
#[derive(Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(super) enum ObjectState {
    Present,
    Absent,
}

#[derive(Deserialize)]
struct StatAnswer {
    state: ObjectState,
}

/// The channel's own word on one object.
pub(super) fn state(stado: &Path, uri: &str) -> Result<ObjectState, Refusal> {
    let output = storage(stado, &["stat", uri, "--json"])?;
    if !output.status.success() {
        return Err(refusal(&output, format!("storage stat failed for {uri}")));
    }
    let answer: StatAnswer = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "storage stat for {uri} answered {:?}, which is not a present or absent object: {error}",
            String::from_utf8_lossy(&output.stdout).trim()
        )
    })?;
    Ok(answer.state)
}

fn get(stado: &Path, uri: &str, destination: &Path) -> Result<(), Refusal> {
    let destination = destination.to_string_lossy();
    let output = storage(stado, &["get", uri, &destination])?;
    if !output.status.success() {
        return Err(refusal(&output, format!("storage get failed for {uri}")));
    }
    Ok(())
}

/// Lowercase-or-uppercase hex of exactly a SHA-256 digest's length, measured
/// from a digest rather than written here.
fn sha256_hex(value: &str) -> bool {
    value.len() == hex::encode(Sha256::digest(b"")).len()
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The fields of the SIGNED manifest the baseline reads. Additive fields are
/// compatible, so unknown ones are not refused; a missing one, a size of
/// zero or a field of the wrong kind is refused by the parse naming it.
#[derive(Deserialize)]
pub(super) struct Manifest {
    artifact_bytes: std::num::NonZeroU64,
    artifact_sha256: String,
    platform: String,
    product: String,
    source_revision: String,
    version: String,
}

/// The SIGNED manifest `release.json`, not the tag train's
/// `release-manifest-<platform>.json`: `/docs/primitives/release` names
/// `release.json` as the commit marker written last.
pub(super) fn manifest(
    stado: &Path,
    version: &str,
    platform: &str,
    root: &Path,
) -> Result<(Manifest, String), Refusal> {
    let uri = format!("{}/release.json", release_base(version, platform));
    let destination = root.join(format!("release-{platform}.json"));
    get(stado, &uri, &destination)?;
    let bytes = std::fs::read(&destination).map_err(|error| {
        format!(
            "the release manifest {uri} fetched to {} cannot be read: {error}",
            destination.display()
        )
    })?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|error| {
        format!("the release manifest {uri} is not a signed release manifest: {error}")
    })?;
    if manifest.product != "stado" || manifest.version != version || manifest.platform != platform {
        return Err(format!(
            "release manifest identity mismatch: {uri} names {} {} {}",
            manifest.product, manifest.version, manifest.platform
        )
        .into());
    }
    if !sha256_hex(&manifest.artifact_sha256) {
        return Err(format!(
            "release manifest digest {:?} is not a SHA-256 digest: {uri}",
            manifest.artifact_sha256
        )
        .into());
    }
    // A git object id of either object format: SHA-1, or SHA-256.
    let revision = &manifest.source_revision;
    if !crate::binary::provenance::is_commit_id(revision) && !sha256_hex(revision) {
        return Err(format!(
            "release manifest source revision {revision:?} is not a git object id: {uri}"
        )
        .into());
    }
    Ok((manifest, uri))
}

/// The command surface of one published release, read from its own binary
/// after the archive matched its manifest by digest AND by size: a payload
/// that lost a chunk decodes into something shorter and perfectly valid.
pub(super) fn surface_from_release(
    stado: &Path,
    version: &str,
    platform: &str,
    root: &Path,
) -> Result<Value, Refusal> {
    use std::os::unix::fs::PermissionsExt;
    let (manifest, manifest_uri) = manifest(stado, version, platform, root)?;
    let archive_uri = format!("{}/release.tar.gz", release_base(version, platform));
    let archive = root.join("release.tar.gz");
    get(stado, &archive_uri, &archive)?;
    let payload = std::fs::read(&archive).map_err(|error| format!("{archive_uri}: {error}"))?;
    if hex::encode(Sha256::digest(&payload)) != manifest.artifact_sha256.to_ascii_lowercase() {
        return Err(format!("release archive differs from its manifest: {archive_uri}").into());
    }
    let size = u64::try_from(payload.len()).map_err(|error| {
        format!("release archive {archive_uri} is larger than a size can say: {error}")
    })?;
    if size != manifest.artifact_bytes.get() {
        return Err(
            format!("release archive is not the size its manifest binds: {archive_uri}").into(),
        );
    }
    let extracted = root.join("release");
    crate::release_control::safe_extract_archive(&payload, &extracted)
        .map_err(|error| format!("{error}: {archive_uri}"))?;
    let binary = extracted.join("stado");
    if !binary.is_file() {
        return Err(format!("release archive contains no stado binary: {archive_uri}").into());
    }
    let mut mode = std::fs::metadata(&binary)
        .map_err(|error| format!("{}: {error}", binary.display()))?
        .permissions();
    mode.set_mode(mode.mode() | crate::primitives::file_mode::widen(nix::libc::S_IXUSR));
    std::fs::set_permissions(&binary, mode)
        .map_err(|error| format!("{}: {error}", binary.display()))?;
    let commands = super::super::surface::of_binary(&binary).map_err(|error| {
        format!("release binary advertised an invalid command surface: {archive_uri}: {error}")
    })?;
    Ok(serde_json::json!({
        "version": version,
        "source": format!("{manifest_uri} published from {}", manifest.source_revision),
        "surface": commands,
    }))
}
