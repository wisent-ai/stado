//! Signed Stado replacement used by `host recover --release`.
//!
//! Recovery deliberately does not use `STADO_API_URL`, a local resolver, or
//! the Stado binary already installed on the target. The control process reads
//! the three immutable signed-release objects from the fixed public emergency
//! origin, verifies them against the already-loaded registry, and carries only
//! the verified binary over the approved SSH channel.

use std::io::Write;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{host_recovery, shlex_quote, DeployError, Runner};
use crate::release_control::{
    self, QualificationStatus, ReleaseArtifactRef, ReleaseControl, ReleaseManifest,
};
use crate::targets::{ComputeTarget, Registry};

pub const RECOVERY_RELEASE_ORIGIN: &str = "https://stado.wisent.com";
const MAX_RELEASE_METADATA_BYTES: usize = 1024 * 1024;

/// Direct reader for the public emergency release route. Production constructs
/// this only with [`canonical`]; the explicit constructor exists for isolated
/// HTTP-level contract tests and is never configured from process environment.
pub struct RecoveryReleaseClient {
    origin: reqwest::Url,
    http: reqwest::Client,
}

impl RecoveryReleaseClient {
    pub fn canonical() -> Result<Self, DeployError> {
        Self::from_origin(RECOVERY_RELEASE_ORIGIN)
    }

    #[doc(hidden)]
    pub fn from_origin(origin: &str) -> Result<Self, DeployError> {
        let origin = reqwest::Url::parse(origin)
            .map_err(|error| DeployError(format!("invalid recovery release origin: {error}")))?;
        if origin.cannot_be_a_base()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(DeployError(
                "recovery release origin must be an absolute URL without credentials, query, or fragment"
                    .to_string(),
            ));
        }
        // Same client the rest of the fleet reads objects with: it trusts
        // `storage.stado.ca_file` and bounds connect and read, so a slow or
        // privately-signed origin fails with its own sentence instead of a bare
        // "error sending request", and a stalled body cannot hang recovery.
        let http = crate::cli::storage::fleet_https_client()
            .map_err(|error| DeployError(format!("recovery release client: {error}")))?;
        Ok(Self { origin, http })
    }

    async fn get(&self, uri: &str) -> Result<Vec<u8>, DeployError> {
        let mut endpoint = self
            .origin
            .join("/api/release/object")
            .map_err(|error| DeployError(format!("invalid recovery release endpoint: {error}")))?;
        endpoint.query_pairs_mut().append_pair("uri", uri);
        let mut response = self.http.get(endpoint).send().await.map_err(|error| {
            DeployError(format!("signed release download failed for {uri}: {error}"))
        })?;
        if !response.status().is_success() {
            return Err(DeployError(format!(
                "signed release object {uri} returned HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RELEASE_METADATA_BYTES as u64)
        {
            return Err(DeployError(format!(
                "signed release object {uri} exceeds the recovery size limit"
            )));
        }
        let mut bytes = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or_default()
                .min(MAX_RELEASE_METADATA_BYTES as u64) as usize,
        );
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            DeployError(format!("signed release download failed for {uri}: {error}"))
        })? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RELEASE_METADATA_BYTES {
                return Err(DeployError(format!(
                    "signed release object {uri} exceeds the recovery size limit"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn download_metadata(
        &self,
        version: &str,
        platform: &str,
    ) -> Result<(Vec<u8>, Vec<u8>), DeployError> {
        let base =
            release_control::release_base("stado", version, platform).map_err(DeployError)?;
        let manifest = self
            .get(&format!(
                "{base}/{}",
                release_control::RELEASE_MANIFEST_NAME
            ))
            .await?;
        let signature = self
            .get(&format!(
                "{base}/{}",
                release_control::RELEASE_SIGNATURE_NAME
            ))
            .await?;
        Ok((manifest, signature))
    }

    async fn download_archive(
        &self,
        uri: &str,
        manifest: &ReleaseManifest,
        destination: &mut std::fs::File,
    ) -> Result<(), DeployError> {
        let mut endpoint = self
            .origin
            .join("/api/release/object")
            .map_err(|error| DeployError(format!("invalid recovery release endpoint: {error}")))?;
        endpoint.query_pairs_mut().append_pair("uri", uri);
        let mut response = self.http.get(endpoint).send().await.map_err(|error| {
            DeployError(format!("signed release download failed for {uri}: {error}"))
        })?;
        if !response.status().is_success() {
            return Err(DeployError(format!(
                "signed release object {uri} returned HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length != manifest.artifact_bytes)
        {
            return Err(DeployError(
                "release archive size differs from its signed manifest".to_string(),
            ));
        }
        let mut received = 0_u64;
        let mut digest = Sha256::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            DeployError(format!("signed release download failed for {uri}: {error}"))
        })? {
            received = received
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| DeployError("release archive size overflowed".to_string()))?;
            if received > manifest.artifact_bytes {
                return Err(DeployError(
                    "release archive exceeds its signed artifact_bytes".to_string(),
                ));
            }
            digest.update(&chunk);
            destination.write_all(&chunk).map_err(|error| {
                DeployError(format!("cannot stage signed release archive: {error}"))
            })?;
        }
        destination
            .flush()
            .and_then(|()| destination.sync_all())
            .map_err(|error| {
                DeployError(format!("cannot commit signed release archive: {error}"))
            })?;
        if received != manifest.artifact_bytes
            || hex::encode(digest.finalize()) != manifest.artifact_sha256
        {
            return Err(DeployError(
                "release archive differs from its signed manifest".to_string(),
            ));
        }
        Ok(())
    }
}

fn step(name: &str, status: &str, detail: impl Into<String>) -> Value {
    let detail = detail.into();
    let mut entry = Map::from_iter([
        ("step".to_string(), json!(name)),
        ("status".to_string(), json!(status)),
    ]);
    if !detail.is_empty() && detail != "-" {
        entry.insert("detail".to_string(), json!(detail));
    }
    Value::Object(entry)
}

fn failed(
    target: &str,
    version: &str,
    steps: Vec<Value>,
    error: impl Into<String>,
    exit_code: i32,
) -> Value {
    json!({
        "target": target,
        "status": "failed",
        "exit_code": exit_code,
        "error": error.into(),
        "release": {"version": version, "steps": steps},
    })
}

fn recovery_control(registry: &Registry) -> Result<ReleaseControl, DeployError> {
    release_control::control(&registry.to_document())
        .map_err(DeployError)?
        .ok_or_else(|| {
            DeployError("registry.release_control is required for release recovery".to_string())
        })
}

fn verify_release_metadata(
    version: &str,
    platform: &str,
    control: &ReleaseControl,
    manifest_bytes: &[u8],
    signature: &[u8],
) -> Result<(ReleaseManifest, ReleaseArtifactRef), DeployError> {
    let base = release_control::release_base("stado", version, platform).map_err(DeployError)?;
    let manifest_uri = format!("{base}/{}", release_control::RELEASE_MANIFEST_NAME);
    let signature_uri = format!("{base}/{}", release_control::RELEASE_SIGNATURE_NAME);
    let archive_uri = format!("{base}/{}", release_control::RELEASE_ARCHIVE_NAME);
    let manifest: ReleaseManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| DeployError(format!("release manifest is invalid: {error}")))?;
    release_control::validate_manifest(&manifest).map_err(DeployError)?;
    if manifest.product != "stado" || manifest.version != version || manifest.platform != platform {
        return Err(DeployError(
            "release manifest identity does not match its object coordinate".to_string(),
        ));
    }
    if manifest.qualification.status != QualificationStatus::Passed {
        return Err(DeployError(format!(
            "release stado {version} {platform} has not passed qualification"
        )));
    }
    let public = control.trusted_keys.get(&manifest.key_id).ok_or_else(|| {
        DeployError(format!(
            "release key {} is not trusted by registry",
            manifest.key_id
        ))
    })?;
    let signature = std::str::from_utf8(signature)
        .map_err(|_| DeployError("release signature is not UTF-8".to_string()))?;
    release_control::verify_manifest(public, &manifest, signature).map_err(DeployError)?;
    let artifact = ReleaseArtifactRef {
        manifest_uri,
        signature_uri,
        archive_uri,
        manifest_sha256: release_control::sha256_bytes(manifest_bytes),
        artifact_sha256: manifest.artifact_sha256.clone(),
        source_revision: manifest.source_revision.clone(),
        key_id: manifest.key_id.clone(),
    };
    Ok((manifest, artifact))
}

const STAGED_STADO_BINARY: &str = "bin/stado";

fn install_script(target: &ComputeTarget, binary: &[u8], digest: &str) -> String {
    let identities = host_recovery::identity_values(target)
        .iter()
        .map(|value| shlex_quote(value))
        .collect::<Vec<_>>()
        .join(" ");
    let payload = BASE64.encode(binary);
    format!(
        r#"set -u
emit() {{ printf 'STADO_RECOVERY_RELEASE\t%s\t%s\t%s\n' "$1" "$2" "$3"; }}
host=$(/bin/hostname -s 2>/dev/null | /usr/bin/tr '[:upper:]' '[:lower:]')
identity_ok=0
for expected in {identities}; do
  short="${{expected%.local}}"
  if [ "$host" = "$expected" ] || [ "$host" = "$short" ]; then identity_ok=1; fi
done
if [ "$identity_ok" -ne 1 ]; then emit install failed "identity_mismatch:$host"; exit 64; fi
if [ "$(/usr/bin/uname -s)" != Darwin ]; then emit install failed unsupported_os; exit 65; fi
for required in /usr/bin/base64 /usr/bin/codesign /usr/bin/openssl /usr/bin/mktemp /bin/chmod /bin/ln /bin/mkdir /bin/mv /bin/rm; do
  if [ ! -x "$required" ]; then emit install failed "missing_${{required##*/}}"; exit 66; fi
done
bin_dir="$HOME/.stado/bin"
active="$bin_dir/stado"
backup="$bin_dir/stado.previous"
backup_pending="$bin_dir/.stado.previous.pending"
rollback_pending="$bin_dir/.stado.rollback.pending"
incoming=""
activated=0
had_previous=0
committed=0
finish() {{
  rc="$1"
  trap - EXIT HUP INT TERM
  if [ "$activated" -eq 1 ] && [ "$committed" -ne 1 ]; then
    if [ "$had_previous" -eq 1 ]; then
      /bin/rm -f "$rollback_pending"
      if /bin/ln "$backup" "$rollback_pending" && /bin/mv -f "$rollback_pending" "$active"; then
        emit rollback restored "$backup"
      else
        emit rollback failed "$backup"
      fi
    else
      /bin/rm -f "$active"
      emit rollback removed no_previous_binary
    fi
  fi
  [ -z "$incoming" ] || /bin/rm -f "$incoming"
  /bin/rm -f "$backup_pending" "$rollback_pending"
  exit "$rc"
}}
trap 'finish $?' EXIT HUP INT TERM
/bin/mkdir -p "$bin_dir"
incoming=$(/usr/bin/mktemp "$bin_dir/.stado.recovery.XXXXXX") || {{ emit install failed cannot_create_staging_file; exit 67; }}
/usr/bin/base64 -D > "$incoming" <<'STADO_RECOVERY_BINARY'
{payload}
STADO_RECOVERY_BINARY
actual=$(/usr/bin/openssl dgst -sha256 -r "$incoming")
actual="${{actual%% *}}"
if [ "$actual" != "{digest}" ]; then emit install failed transfer_sha256_mismatch; exit 68; fi
/bin/chmod 755 "$incoming" || {{ emit install failed cannot_make_executable; exit 69; }}
/usr/bin/codesign -f -s - "$incoming" >/dev/null 2>&1 || {{ emit install failed codesign_failed; exit 69; }}
/usr/bin/codesign --verify "$incoming" >/dev/null 2>&1 || {{ emit install failed codesign_verification_failed; exit 69; }}
if [ -e "$active" ] || [ -L "$active" ]; then
  if [ -L "$active" ] || [ ! -f "$active" ]; then emit backup failed active_binary_is_not_regular; exit 69; fi
  /bin/rm -f "$backup_pending"
  /bin/ln "$active" "$backup_pending" || {{ emit backup failed cannot_link_previous; exit 69; }}
  /bin/mv -f "$backup_pending" "$backup" || {{ emit backup failed cannot_preserve_previous; exit 69; }}
  had_previous=1
  emit backup ok "$backup"
else
  emit backup absent no_previous_binary
fi
activated=1
/bin/mv -f "$incoming" "$active" || {{ emit install failed atomic_rename_failed; exit 69; }}
incoming=""
emit install ok "$active"
if "$active" resolver --help >/dev/null 2>&1; then
  emit resolver ok "$active resolver --help"
else
  emit resolver failed "$active resolver --help"
  exit 70
fi
committed=1
exit 0
"#
    )
}

fn append_remote_steps(steps: &mut Vec<Value>, stdout: &str) {
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if let ["STADO_RECOVERY_RELEASE", name, status, detail] = fields.as_slice() {
            steps.push(step(name, status, *detail));
        }
    }
}

/// Replace Stado from the signed emergency channel, verify the new resolver,
/// and only then execute the established host recovery procedure.
pub async fn recover_with_client(
    registry: &Registry,
    target_name: &str,
    version: &str,
    client: &RecoveryReleaseClient,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let target = super::host_channel::resolve_target(registry, target_name)?;
    if !target.has_ssh_connection() {
        return Err(DeployError(format!(
            "target {target_name:?} has no registry-managed ssh destination"
        )));
    }
    if target
        .weles
        .as_ref()
        .is_some_and(|policy| policy.actions.iter().any(|action| action == "*"))
    {
        return Err(DeployError(format!(
            "target {target_name:?} carries forbidden wildcard recovery state"
        )));
    }
    if !super::host_release::is_exact_semver(version) {
        return Err(DeployError(format!(
            "{version:?} is not an exact semantic version"
        )));
    }
    let control = recovery_control(registry)?;
    let platform = target.release_platform.as_str();
    let mut steps = Vec::new();
    let (manifest_bytes, signature) = match client.download_metadata(version, platform).await {
        Ok(objects) => {
            steps.push(step("download", "ok", "release.json, release.sig"));
            objects
        }
        Err(error) => {
            let detail = error.to_string();
            steps.push(step("download", "failed", &detail));
            return Ok(failed(target_name, version, steps, detail, 1));
        }
    };
    let (manifest, artifact) =
        match verify_release_metadata(version, platform, &control, &manifest_bytes, &signature) {
            Ok(verified) => verified,
            Err(error) => {
                let detail = error.to_string();
                steps.push(step("verify", "failed", &detail));
                return Ok(failed(target_name, version, steps, detail, 1));
            }
        };

    let mut archive = tempfile::NamedTempFile::new()
        .map_err(|error| DeployError(format!("cannot create private release archive: {error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        archive
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| DeployError(format!("cannot protect release archive: {error}")))?;
    }
    if let Err(error) = client
        .download_archive(&artifact.archive_uri, &manifest, archive.as_file_mut())
        .await
    {
        let detail = error.to_string();
        if detail.contains("download failed") || detail.contains("returned HTTP") {
            steps[0] = step("download", "failed", &detail);
        } else {
            steps.push(step("verify", "failed", &detail));
        }
        return Ok(failed(target_name, version, steps, detail, 1));
    }
    steps[0] = step(
        "download",
        "ok",
        "release.json, release.sig, release.tar.gz",
    );
    let extracted = tempfile::tempdir().map_err(|error| {
        DeployError(format!("cannot create release staging directory: {error}"))
    })?;
    let release_root = extracted.path().join("release");
    if let Err(error) = release_control::safe_extract_archive_file(
        archive.path(),
        manifest.artifact_bytes,
        &release_root,
    ) {
        steps.push(step("verify", "failed", &error));
        return Ok(failed(target_name, version, steps, error, 1));
    }
    let binary_path = release_root.join(STAGED_STADO_BINARY);
    let metadata = match std::fs::symlink_metadata(&binary_path) {
        Ok(metadata) if metadata.file_type().is_file() && metadata.len() > 0 => metadata,
        Ok(_) => {
            let detail = "signed release member bin/stado is not a non-empty regular file";
            steps.push(step("verify", "failed", detail));
            return Ok(failed(target_name, version, steps, detail, 1));
        }
        Err(error) => {
            let detail = format!("signed release member bin/stado is unavailable: {error}");
            steps.push(step("verify", "failed", &detail));
            return Ok(failed(target_name, version, steps, detail, 1));
        }
    };
    let _ = metadata;
    let binary = std::fs::read(&binary_path)
        .map_err(|error| DeployError(format!("cannot read signed release binary: {error}")))?;
    let binary_digest = release_control::sha256_bytes(&binary);
    steps.push(step(
        "verify",
        "ok",
        format!(
            "key_id={}; manifest_sha256={}; archive_sha256={}",
            artifact.key_id, artifact.manifest_sha256, artifact.artifact_sha256
        ),
    ));

    let installed = match super::host_channel::run_script_with_timeout(
        target,
        &install_script(target, &binary, &binary_digest),
        Duration::from_secs(host_recovery::TIMEOUT_SECONDS),
        runner,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            let detail = error.to_string();
            steps.push(step("install", "failed", &detail));
            return Ok(failed(target_name, version, steps, detail, 1));
        }
    };
    append_remote_steps(&mut steps, &installed.stdout);
    if !installed.ok() {
        let detail = steps
            .iter()
            .rev()
            .find(|entry| entry.get("status").and_then(Value::as_str) == Some("failed"))
            .and_then(|entry| entry.get("detail").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| {
                installed
                    .detail()
                    .lines()
                    .next_back()
                    .unwrap_or("remote release installation failed")
                    .chars()
                    .take(300)
                    .collect::<String>()
            });
        if !steps
            .iter()
            .any(|entry| entry.get("status").and_then(Value::as_str) == Some("failed"))
        {
            steps.push(step("install", "failed", &detail));
        }
        return Ok(failed(target_name, version, steps, detail, installed.code));
    }

    let mut report =
        match host_recovery::recover_host_with_registry(registry, target_name, runner).await {
            Ok(report) => report,
            Err(error) => {
                let detail = error.to_string();
                steps.push(step("recovery", "failed", &detail));
                return Ok(failed(target_name, version, steps, detail, 1));
            }
        };
    let recovered = report.get("status").and_then(Value::as_str) == Some(host_recovery::STATUS_OK);
    steps.push(step(
        "recovery",
        if recovered { "ok" } else { "failed" },
        report
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
    ));
    report["release"] = json!({"version": version, "steps": steps});
    Ok(report)
}

pub async fn recover(
    registry: &Registry,
    target_name: &str,
    version: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let client = RecoveryReleaseClient::canonical()?;
    recover_with_client(registry, target_name, version, &client, runner).await
}
