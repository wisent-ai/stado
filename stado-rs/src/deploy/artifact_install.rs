//! Materialise a published artifact onto a host, so a unit can point at a
//! version rather than at whatever happens to be on disk.
//!
//! `stado service deploy --from PATH` takes the absolute path, on the target
//! host, of the program the unit runs. That is deliberate — the command
//! manages units, not contents — but it leaves a gap nothing else in the pack
//! fills: no system owns getting a build onto a host. The consequence is
//! visible on any machine that has been running a while. Service directories
//! accumulate `current` as a plain copied directory beside hand-named backups
//! like `current.before-<change>-<timestamp>`, there is no version identity to
//! report, and nothing can say which build is running or what it is compatible
//! with. On 2026-08-04 a Skarbiec rebuilt in place began answering
//! `400 field required` to clients that had not moved with it, and took out a
//! health beacon and a gateway on the same host; no lineage existed to consult
//! because neither side was a published artifact.
//!
//! The two halves of the answer already exist. `stado artifact` publishes
//! immutable versioned manifests with aliases and lineage, and `service deploy`
//! renders a unit around a path. This module is the join: resolve an alias to
//! an immutable version, place exactly that version on the host under a path
//! that names it, verify the digest the manifest declares, and move `current`
//! onto it atomically. The unit then points at `current`, so a rollback is a
//! relink rather than a rebuild.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use super::{host_channel, shlex_quote, DeployError, Runner};
use crate::artifacts_models::{ArtifactManifest, ArtifactRef};
use crate::targets::ComputeTarget;

/// Where a materialised service version lands, relative to the host's home.
pub const SERVICES_ROOT: &str = ".stado/services";

/// Everything the caller needs after a successful install: the path the unit
/// must run, and the immutable version now behind `current`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledArtifact {
    pub program_path: String,
    pub version: String,
    pub sha256: String,
}

/// A service name that is safe as a path segment. Deliberately stricter than
/// the unit-name rule: this value is interpolated into a remote shell script,
/// and a name that needs quoting to be safe is a name that should be refused.
fn validate_service_name(name: &str) -> Result<(), DeployError> {
    let safe = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.');
    if safe {
        Ok(())
    } else {
        Err(DeployError(format!(
            "service name {name:?} must be lowercase letters, digits, '.', '-' or '_'"
        )))
    }
}

/// The manifest's primary location, which is the copy a consumer is meant to
/// read. A manifest without one is a publication bug rather than a transfer
/// failure, so it is reported as such.
fn primary_location(
    manifest: &ArtifactManifest,
) -> Result<&crate::artifacts_models::ArtifactLocation, DeployError> {
    manifest
        .locations
        .iter()
        .find(|location| location.role == "primary")
        .ok_or_else(|| {
            DeployError(format!(
                "artifact {} declares no primary location",
                manifest.ref_
            ))
        })
}

/// The version segment a materialised artifact is stored under.
///
/// Taken from the resolved reference rather than from an alias, so the path on
/// disk names the immutable version even when the operator deployed
/// `service@stable`. That is the whole point: `current` may move, the version
/// directory beside it may not.
fn version_segment(reference: &ArtifactRef) -> Result<String, DeployError> {
    let version = reference.version.trim();
    let safe = !version.is_empty()
        && version.len() <= 128
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !version.starts_with('.');
    if safe {
        Ok(version.to_string())
    } else {
        Err(DeployError(format!(
            "artifact version {version:?} is not usable as a path segment"
        )))
    }
}

/// The script that does the work on the host.
///
/// Written so that a failure at any step leaves the previous `current` intact:
/// the download lands in the version directory, the digest is checked there,
/// and only a verified file causes the symlink to move. `ln -sfn` through a
/// temporary name makes the final swap atomic, so a reader never observes a
/// `current` that points at nothing.
const INSTALL_BODY: &str = r#"
set -eu
root="$HOME/@SERVICES_ROOT@/@NAME@"
version_dir="$root/@VERSION@"
program="$version_dir/@NAME@"
mkdir -p "$version_dir"

uri=@URI@
# The fleet's own release channel first: stado:// resolves through whatever
# object store this host is configured with, so a release does not depend on
# any one vendor being reachable. An https location is still a location -- it
# is how something published outside the fleet arrives -- and anything else is
# refused by name rather than handed to a command that means something else.
if [ -x "$HOME/.stado/bin/stado" ]; then
  stado_bin="$HOME/.stado/bin/stado"
else
  stado_bin="$(command -v stado || true)"
fi
fetch_object() {
  case "$uri" in
    stado://*)
      if [ -z "$stado_bin" ]; then
        echo "STADO_STATUS=failed"
        echo "STADO_DETAIL=$uri needs stado on this host to read the release channel"
        exit 1
      fi
      "$stado_bin" storage cat "$uri" > "$1"
      ;;
    https://*)
      /usr/bin/curl -fsSL --retry 3 "$uri" -o "$1"
      ;;
    *)
      echo "STADO_STATUS=failed"
      echo "STADO_DETAIL=artifact location $uri is neither the fleet release channel nor https"
      exit 1
      ;;
  esac
}
if [ ! -f "$program" ]; then
  fetch_object "$program"
fi

actual="$(shasum -a 256 "$program" | awk '{print $1}')"
if [ "$actual" != "@SHA256@" ]; then
  rm -f "$program"
  echo "STADO_STATUS=failed"
  echo "STADO_DETAIL=digest mismatch: manifest says @SHA256@, downloaded $actual"
  exit 1
fi

chmod u+x "$program"
ln -sfn "$version_dir" "$root/.current.new"
mv -f "$root/.current.new" "$root/current"
echo "STADO_STATUS=installed"
echo "STADO_DETAIL=$program"
"#;

const INSTALL_ARCHIVE_BODY: &str = r#"
set -eu
root="$HOME/@SERVICES_ROOT@/@NAME@"
version_dir="$root/@VERSION@"
dest="$version_dir/@SUBDIR@"
archive="$version_dir/.artifact-download"
mkdir -p "$dest"

uri=@URI@
# The fleet's own release channel first: stado:// resolves through whatever
# object store this host is configured with, so a release does not depend on
# any one vendor being reachable. An https location is still a location -- it
# is how something published outside the fleet arrives -- and anything else is
# refused by name rather than handed to a command that means something else.
if [ -x "$HOME/.stado/bin/stado" ]; then
  stado_bin="$HOME/.stado/bin/stado"
else
  stado_bin="$(command -v stado || true)"
fi
fetch_object() {
  case "$uri" in
    stado://*)
      if [ -z "$stado_bin" ]; then
        echo "STADO_STATUS=failed"
        echo "STADO_DETAIL=$uri needs stado on this host to read the release channel"
        exit 1
      fi
      "$stado_bin" storage cat "$uri" > "$1"
      ;;
    https://*)
      /usr/bin/curl -fsSL --retry 3 "$uri" -o "$1"
      ;;
    *)
      echo "STADO_STATUS=failed"
      echo "STADO_DETAIL=artifact location $uri is neither the fleet release channel nor https"
      exit 1
      ;;
  esac
}
if [ ! -f "$archive" ]; then
  fetch_object "$archive"
fi

actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
if [ "$actual" != "@SHA256@" ]; then
  rm -f "$archive"
  echo "STADO_STATUS=failed"
  echo "STADO_DETAIL=digest mismatch: manifest says @SHA256@, downloaded $actual"
  exit 1
fi

tar -xzf "$archive" -C "$dest"
rm -f "$archive"
ln -sfn "$version_dir" "$root/.current.new"
mv -f "$root/.current.new" "$root/current"
echo "STADO_STATUS=installed"
echo "STADO_DETAIL=$dest"
"#;

/// Place one artifact version on the host and point `current` at it.
///
/// Returns the path the unit should run: always through `current`, never the
/// version directory, so a later install moves every unit forward without
/// re-rendering any of them.
pub async fn install_artifact(
    target: &ComputeTarget,
    name: &str,
    manifest: &ArtifactManifest,
    runner: &Runner,
) -> Result<InstalledArtifact, DeployError> {
    validate_service_name(name)?;
    let version = version_segment(&manifest.ref_)?;
    let location = primary_location(manifest)?;
    if location.sha256.trim().is_empty() {
        return Err(DeployError(format!(
            "artifact {} declares no sha256 for its primary location; \
             an unverifiable download must not become a running unit",
            manifest.ref_
        )));
    }

    // A bundle is not a program. When the manifest says the location is an
    // archive, the verified download is unpacked into the version directory
    // instead of becoming the executable itself -- brama ships its launcher,
    // its entitlements router and its config beside the binary, and installing
    // only the tarball would leave `current` pointing at a tarball.
    let archive = manifest.labels.get("archive").map(String::as_str);
    let subdir = manifest
        .labels
        .get("extract_subdir")
        .map(String::as_str)
        .unwrap_or_default();
    if subdir.contains("..") || subdir.starts_with('/') {
        return Err(DeployError(format!(
            "artifact {} declares an unusable extract_subdir {subdir:?}",
            manifest.ref_
        )));
    }
    let body = match archive {
        Some("tar.gz" | "tgz") => INSTALL_ARCHIVE_BODY,
        Some(other) => {
            return Err(DeployError(format!(
                "artifact {} declares an unsupported archive format {other:?}",
                manifest.ref_
            )))
        }
        None => INSTALL_BODY,
    };
    let script = body
        .replace("@SERVICES_ROOT@", SERVICES_ROOT)
        .replace("@NAME@", name)
        .replace("@VERSION@", &version)
        .replace("@SUBDIR@", subdir)
        .replace("@URI@", &shlex_quote(&location.uri))
        .replace("@SHA256@", &location.sha256);
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: could not install artifact {}: {}",
            target.name,
            manifest.ref_,
            host_channel::last_error_line(&output, "install failed")
        )));
    }

    Ok(InstalledArtifact {
        program_path: format!("$HOME/{SERVICES_ROOT}/{name}/current/{name}"),
        version,
        sha256: location.sha256.clone(),
    })
}

/// The same path the install reports, resolved for a unit definition.
///
/// `deploy` validates an absolute path, and `$HOME` is not one, so the caller
/// needs the expanded form. The home directory comes from the host rather than
/// from this machine: a target's account is its own business.
pub async fn resolve_program_path(
    target: &ComputeTarget,
    name: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    validate_service_name(name)?;
    let script = format!(
        "set -eu\necho \"STADO_HOME=$HOME\"\n_={}\n",
        STANDARD.encode(name.as_bytes())
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let home = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_HOME="))
        .map(str::trim)
        .filter(|value| value.starts_with('/'))
        .ok_or_else(|| {
            DeployError(format!(
                "{}: could not resolve the home directory for the unit path",
                target.name
            ))
        })?;
    Ok(format!("{home}/{SERVICES_ROOT}/{name}/current/{name}"))
}
