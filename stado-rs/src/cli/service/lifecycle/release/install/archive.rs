//! Installing a release archive beside the immutable version it names, and
//! the host script that does it.

use super::*;

/// Extract beside the immutable version and confirm the declared executable
/// before atomically replacing `current`. Never remove an installed version
/// while extracting its replacement. The archive is laid out below the
/// target's own platform directory ([`service_catalog::platform_directory`]),
/// the directory the catalog's `$STADO_PLATFORM` names, so `required` is
/// relative to the version directory and starts with it.
pub(crate) async fn install_from_archive(
    target: &crate::targets::ComputeTarget,
    directory: &str,
    path: &str,
    required: &str,
    runner: &crate::deploy::Runner,
) -> Result<(crate::deploy::artifact_install::InstalledArtifact, bool), CmdError> {
    let bytes = std::fs::read(path)?;
    let digest = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        hex::encode(hasher.finalize())
    };
    // The version directory is named for the archive's whole digest, so two
    // archives can never share one.
    let version = format!("sha256-{digest}");
    let staged = format!(".stado/.{directory}-{version}.tar.gz");
    let platform_dir = crate::deploy::service_catalog::platform_directory(&target.release_platform);

    if crate::deploy::host_channel::target_is_this_host(target) {
        let home = crate::config_file::home().map_err(|cause| {
            CmdError::click(format!("{cause}, so the staging path is unknown"))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        let destination = std::path::Path::new(&home).join(&staged);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(path, destination)?;
    } else {
        let connection = host_channel::select_ssh_connection(target, runner)
            .await
            .map_err(CmdError::from)?;
        let ssh_target = connection.destination;
        let prepare = host_channel::run_script(
            target,
            "set -euo pipefail\n/bin/mkdir -p \"$HOME/.stado\"\n/bin/chmod u=rwx,go= \"$HOME/.stado\"\n",
            runner,
        )
        .await
        .map_err(CmdError::from)?;
        if !prepare.ok() {
            return Err(CmdError::click(format!(
                "{}: cannot prepare the staging directory",
                target.name
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        let mut options = host_channel::ssh_options(ssh_target);
        options.pop();
        let mut argv = vec!["scp".to_string(), "-q".to_string()];
        if let Some((_program, flags)) = options.split_first() {
            argv.extend(flags.iter().cloned());
        }
        argv.push(path.to_string());
        argv.push(format!("{ssh_target}:{staged}"));
        let key = crate::deploy::host_access::ssh_key::materialize(target.channel_key())
            .await
            .map_err(CmdError::from)?;
        let argv = crate::deploy::host_access::ssh_key::add_identity(argv, &key)
            .map_err(CmdError::from)?;
        let copy = runner(crate::deploy::CommandSpec::new(argv))
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "{}: cannot run scp to deliver the archive: {error}",
                    target.name
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
        if !copy.ok() {
            return Err(CmdError::click(format!(
                "{}: cannot deliver the archive: {}",
                target.name,
                copy.detail()
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
    }

    let script = format!(
        "set -euo pipefail\nname={}\nversion={}\nexpected={}\nstaged={}\nrequired={}\nplatform_dir={}\n{ARCHIVE_INSTALL_BODY}",
        crate::deploy::shlex_quote(directory),
        crate::deploy::shlex_quote(&version),
        crate::deploy::shlex_quote(&digest),
        crate::deploy::shlex_quote(&staged),
        crate::deploy::shlex_quote(required),
        crate::deploy::shlex_quote(platform_dir),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(CmdError::from)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "the archive did not install")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let already_active = output
        .stdout
        .lines()
        .any(|line| line == "STADO_SERVICE_ARCHIVE already_active");
    Ok((
        crate::deploy::artifact_install::InstalledArtifact {
            program_path: format!("$HOME/.stado/services/{directory}/current"),
            version,
            sha256: digest,
        },
        already_active,
    ))
}

const ARCHIVE_INSTALL_BODY: &str = r#"
root="$HOME/.stado/services/$name"
version_dir="$root/$version"
archive="$HOME/$staged"
incoming="$root/.$version.incoming.$$"
link="$root/.current.new.$$"
trap 'rm -f "$archive" "$link"; rm -rf "$incoming"' EXIT

[ -s "$archive" ] || { printf '%s\n' 'delivered archive is missing or empty' >&2; false; }
# The digest is SHA-256 (https://csrc.nist.gov/pubs/fips/180-4/upd1/final),
# the one Stado computed over the archive it sent.
read -r actual _ < <(/usr/bin/shasum -a 256 "$archive")
if [ "$actual" != "$expected" ]; then
  printf '%s\n' "digest mismatch: expected $expected, delivered $actual" >&2
  false
fi

/bin/mkdir -p "$incoming/$platform_dir"
/usr/bin/tar -xzf "$archive" -C "$incoming/$platform_dir"
if [ ! -f "$incoming/$required" ] || [ ! -x "$incoming/$required" ]; then
  printf '%s\n' "archive does not carry the declared executable $required; current is unchanged" >&2
  false
fi
if [ -e "$version_dir" ]; then
  if ! /usr/bin/diff -qr "$incoming" "$version_dir" >/dev/null; then
    printf '%s\n' "existing immutable version $version differs from its archive; current is unchanged" >&2
    false
  fi
  [ -x "$version_dir/$required" ] || {
    printf '%s\n' "existing immutable version $version has no executable $required; current is unchanged" >&2
    false
  }
  rm -rf "$incoming"
else
  # $version_dir does not exist, so this is one rename(2) of the directory.
  /bin/mv "$incoming" "$version_dir"
fi

if [ -L "$root/current" ] &&
   [ "$(/usr/bin/readlink "$root/current")" = "$version_dir" ]; then
  trap - EXIT
  rm -f "$archive"
  printf '%s\n' 'STADO_SERVICE_ARCHIVE already_active'
  exit
fi

# `current` is a directory here on some hosts and a symlink on others; either
# way the previous one is kept beside the new version rather than deleted, so a
# rollback is a rename.
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.before-$version.$$"
fi
/bin/ln -s "$version_dir" "$link"
# -h: when `current` is a symlink to a directory, replace the link itself with
# one rename(2) instead of moving the new link into the old version.
/bin/mv -fh "$link" "$root/current"
trap - EXIT
rm -f "$archive"
printf '%s\n' "$version_dir"
"#;

pub(crate) const ROLLBACK_BODY: &str = r#"
root="$HOME/.stado/services/$name"
target="$root/$version"
[ -d "$target" ] || { printf '%s\n' "no version directory $version on this host" >&2; false; }
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.replaced-$(/bin/date -u +%Y%m%dT%H%M%SZ)"
else
  rm -f "$root/current"
fi
/bin/ln -sfn "$target" "$root/.current.new"
/bin/mv -f "$root/.current.new" "$root/current"
printf '%s\n' "$target"
"#;
