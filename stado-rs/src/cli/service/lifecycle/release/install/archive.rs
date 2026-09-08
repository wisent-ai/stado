//! Installing a release archive beside the immutable version it names, and
//! the host script that does it.

use super::*;

/// Extract beside the immutable version and confirm the declared executable
/// before atomically replacing `current`. Never remove an installed version
/// while extracting its replacement.
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
    let version = format!("sha256-{}", &digest[..usize::from(12u8)]);
    let staged = format!(".stado/.{directory}-{version}.tar.gz");

    if crate::deploy::host_channel::target_is_this_host(target) {
        let home = std::env::var("HOME")
            .map_err(|_| CmdError::click("HOME is not set, so the staging path is unknown"))?;
        let destination = std::path::Path::new(&home).join(&staged);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(path, destination)?;
    } else {
        let connection = host_channel::select_ssh_connection(target, runner)
            .await
            .map_err(click)?;
        let ssh_target = connection.destination;
        let prepare = host_channel::run_script(
            target,
            "set -euo pipefail\n/bin/mkdir -p \"$HOME/.stado\"\n/bin/chmod 700 \"$HOME/.stado\"\n",
            runner,
        )
        .await
        .map_err(click)?;
        if !prepare.ok() {
            return Err(CmdError::click(format!(
                "{}: cannot prepare the staging directory",
                target.name
            )));
        }
        let mut options = host_channel::ssh_options(ssh_target);
        options.pop();
        let mut argv = vec!["scp".to_string(), "-q".to_string()];
        argv.extend(options.into_iter().skip(usize::from(true)));
        argv.push(path.to_string());
        argv.push(format!("{ssh_target}:{staged}"));
        let key = crate::deploy::ssh_key::materialize(target.channel_key())
            .await
            .map_err(click)?;
        let argv = crate::deploy::ssh_key::add_identity(argv, &key).map_err(click)?;
        let copy = runner(crate::deploy::CommandSpec::new(argv))
            .await
            .map_err(CmdError::click)?;
        if !copy.ok() {
            return Err(CmdError::click(format!(
                "{}: cannot deliver the archive: {}",
                target.name,
                copy.detail()
            )));
        }
    }

    let script = format!(
        "set -euo pipefail\nname={}\nversion={}\nexpected={}\nstaged={}\nrequired={}\n{ARCHIVE_INSTALL_BODY}",
        crate::deploy::shlex_quote(directory),
        crate::deploy::shlex_quote(&version),
        crate::deploy::shlex_quote(&digest),
        crate::deploy::shlex_quote(&staged),
        crate::deploy::shlex_quote(required),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "the archive did not install")
        )));
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

[ -s "$archive" ] || { printf '%s\n' 'delivered archive is missing or empty' >&2; exit 1; }
actual="$(/usr/bin/shasum -a 256 "$archive" | /usr/bin/awk '{print $1}')"
if [ "$actual" != "$expected" ]; then
  printf '%s\n' "digest mismatch: expected $expected, delivered $actual" >&2
  exit 1
fi

/bin/mkdir -p "$incoming/darwin-arm"
/usr/bin/tar -xzf "$archive" -C "$incoming/darwin-arm"
if [ ! -f "$incoming/$required" ] || [ ! -x "$incoming/$required" ]; then
  printf '%s\n' "archive does not carry the declared executable $required; current is unchanged" >&2
  exit 1
fi
if [ -e "$version_dir" ]; then
  if ! /usr/bin/diff -qr "$incoming" "$version_dir" >/dev/null; then
    printf '%s\n' "existing immutable version $version differs from its archive; current is unchanged" >&2
    exit 1
  fi
  [ -x "$version_dir/$required" ] || {
    printf '%s\n' "existing immutable version $version has no executable $required; current is unchanged" >&2
    exit 1
  }
  rm -rf "$incoming"
else
  /usr/bin/python3 - "$incoming" "$version_dir" <<'PY'
import os, sys
os.rename(sys.argv[1], sys.argv[2])
PY
fi

if [ -L "$root/current" ] &&
   [ "$(/usr/bin/readlink "$root/current")" = "$version_dir" ]; then
  trap - EXIT
  rm -f "$archive"
  printf '%s\n' 'STADO_SERVICE_ARCHIVE already_active'
  exit 0
fi

# `current` is a directory here on some hosts and a symlink on others; either
# way the previous one is kept beside the new version rather than deleted, so a
# rollback is a rename.
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.before-$version.$$"
fi
/bin/ln -s "$version_dir" "$link"
/usr/bin/python3 - "$link" "$root/current" <<'PY'
import os, sys
os.replace(sys.argv[1], sys.argv[2])
PY
trap - EXIT
rm -f "$archive"
printf '%s\n' "$version_dir"
"#;

pub(crate) const ROLLBACK_BODY: &str = r#"
root="$HOME/.stado/services/$name"
target="$root/$version"
[ -d "$target" ] || { printf '%s\n' "no version directory $version on this host" >&2; exit 1; }
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.replaced-$(/bin/date -u +%Y%m%dT%H%M%SZ)"
else
  rm -f "$root/current"
fi
/bin/ln -sfn "$target" "$root/.current.new"
/bin/mv -f "$root/.current.new" "$root/current"
printf '%s\n' "$target"
"#;
