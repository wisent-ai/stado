//! What the baseline generator asks the release channel, through the Stado
//! binary it was handed: a coordinate's state, the signed manifest, and the
//! archive that manifest attests, verified by digest and by byte count.

use std::path::Path;
use std::process::{Command, Output};

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

/// The channel's own word on one object: `present`, `absent`, or whatever
/// else it said, which the caller refuses.
pub(super) fn state(stado: &Path, uri: &str) -> Result<String, Refusal> {
    let output = storage(stado, &["stat", uri, "--json"])?;
    if !output.status.success() {
        return Err(refusal(&output, format!("storage stat failed for {uri}")));
    }
    let document: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| format!("storage stat returned invalid JSON for {uri}"))?;
    Ok(document
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string())
}

fn get(stado: &Path, uri: &str, destination: &Path) -> Result<(), Refusal> {
    let destination = destination.to_string_lossy();
    let output = storage(stado, &["get", uri, &destination])?;
    if !output.status.success() {
        return Err(refusal(&output, format!("storage get failed for {uri}")));
    }
    Ok(())
}

fn hex_of(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The SIGNED manifest `release.json`, not the tag train's
/// `release-manifest-<platform>.json`: `/docs/primitives/release` names
/// `release.json` as the commit marker written last. A required subset of
/// fields is checked, not an exact set, so an additive field is compatible.
pub(super) fn manifest(
    stado: &Path,
    version: &str,
    platform: &str,
    root: &Path,
) -> Result<(Value, String), Refusal> {
    let uri = format!("{}/release.json", release_base(version, platform));
    let destination = root.join(format!("release-{platform}.json"));
    get(stado, &uri, &destination)?;
    let value: Value = std::fs::read(&destination)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| format!("release channel returned invalid JSON for {uri}"))?;
    let required = [
        "artifact_bytes",
        "artifact_sha256",
        "platform",
        "product",
        "source_revision",
        "version",
    ];
    if !value.is_object() || required.iter().any(|field| value.get(field).is_none()) {
        return Err(format!("signed release manifest is missing required fields: {uri}").into());
    }
    let text = |field: &str| value[field].as_str().unwrap_or("").to_string();
    if value["product"] != "stado" || value["version"] != version || value["platform"] != platform {
        return Err(format!("release manifest identity mismatch: {uri}").into());
    }
    if !hex_of(&text("artifact_sha256"), &[64]) {
        return Err(format!("release manifest digest is invalid: {uri}").into());
    }
    if !value["artifact_bytes"]
        .as_u64()
        .is_some_and(|size| size > 0)
    {
        return Err(format!("release manifest artifact size is invalid: {uri}").into());
    }
    if !hex_of(&text("source_revision"), &[40, 64]) {
        return Err(format!("release manifest source revision is invalid: {uri}").into());
    }
    Ok((value, uri))
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
    let expected = manifest["artifact_sha256"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    if hex::encode(Sha256::digest(&payload)) != expected {
        return Err(format!("release archive differs from its manifest: {archive_uri}").into());
    }
    if Some(payload.len() as u64) != manifest["artifact_bytes"].as_u64() {
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
        .map_err(|error| error.to_string())?
        .permissions();
    mode.set_mode(mode.mode() | 0o100);
    std::fs::set_permissions(&binary, mode).map_err(|error| error.to_string())?;
    let commands = super::super::surface::of_binary(&binary).map_err(|error| {
        format!("release binary advertised an invalid command surface: {archive_uri}: {error}")
    })?;
    let revision = manifest["source_revision"].as_str().unwrap_or("");
    Ok(serde_json::json!({
        "version": version,
        "source": format!("{manifest_uri} published from {revision}"),
        "surface": commands,
    }))
}
