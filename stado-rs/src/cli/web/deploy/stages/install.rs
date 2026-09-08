//! The install root: one published release archive turned into the directory
//! the unit's program path names, and the program path itself.

use crate::cli::web::deploy::{click, marker, INSTALL_TIMEOUT};
use crate::cli::web::LAUNCHER;
use crate::cli::CmdError;
use crate::deploy::{host_channel, service_catalog, Runner};
use crate::targets::ComputeTarget;

/// Place one published web release into the product's install root, verify
/// both archives against the digests that declare them, and point `current`
/// at the result.
///
/// Order is the whole design, and it is the order
/// [`crate::deploy::host_release`] states: nothing touches `current` until
/// both digests have matched and the launcher has been found. A failed fetch,
/// a short body, a tampered inner tarball or a tarball with no launcher in it
/// each leave the running release exactly where it was, because the running
/// release has not been opened.
///
/// The inner sidecar is not belt-and-braces. The release archive's digest is
/// the release plane's statement about the release archive; the sidecar is the
/// build's statement about the tarball the unit actually runs, and they are
/// produced by different steps on different machines. Checking only the outer
/// one would accept a release archive that was assembled correctly around a
/// web tarball the build wrote badly.
pub(in crate::cli::web::deploy) const WEB_INSTALL_BODY: &str = r#"
refuse() {
  printf '%s\n' "$1" >&2
  exit 1
}

digest() {
  if command -v /usr/bin/shasum >/dev/null 2>&1; then
    /usr/bin/shasum -a 256 "$1" | /usr/bin/awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | /usr/bin/awk '{print $1}'
  else
    refuse 'this host has neither shasum nor sha256sum, so nothing here can be verified'
  fi
}

root="$HOME/.stado/services/$name"
version_dir="$root/$version"
release_dir="$version_dir/.release"
download="$version_dir/.release.tar.gz"

/bin/mkdir -p "$version_dir" || refuse "cannot create $version_dir"
/bin/rm -rf "$release_dir"
/bin/mkdir -p "$release_dir" || refuse "cannot create $release_dir"

if [ -x "$HOME/.stado/bin/stado" ]; then
  stado_bin="$HOME/.stado/bin/stado"
else
  stado_bin="$(command -v stado || true)"
fi

case "$archive_uri" in
  stado://*)
    [ -n "$stado_bin" ] || refuse "$archive_uri needs stado on this host to read the release channel"
    "$stado_bin" storage cat "$archive_uri" > "$download" \
      || refuse "the release channel did not serve $archive_uri"
    ;;
  https://*)
    /usr/bin/curl -fsSL --retry 3 "$archive_uri" -o "$download" \
      || refuse "$archive_uri could not be fetched"
    ;;
  *)
    refuse "release location $archive_uri is neither the fleet release channel nor https"
    ;;
esac

observed="$(digest "$download")"
if [ "$observed" != "$expected" ]; then
  /bin/rm -f "$download"
  refuse "release archive digest mismatch: the manifest declares $expected, the host received $observed"
fi

/usr/bin/tar -xzf "$download" -C "$release_dir" || refuse 'the release archive did not unpack'
/bin/rm -f "$download"

set -- "$release_dir"/*-web.tar.gz
if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
  refuse "the release archive stages $# web tarballs; a web release stages exactly one *-web.tar.gz"
fi
inner="$1"
sidecar="$inner.sha256"
[ -f "$sidecar" ] || refuse "the release archive carries no digest sidecar beside $(/usr/bin/basename "$inner")"
declared="$(/usr/bin/awk '{print $1; exit}' "$sidecar")"
[ -n "$declared" ] || refuse 'the digest sidecar is empty'
observed="$(digest "$inner")"
if [ "$declared" != "$observed" ]; then
  refuse "web tarball digest mismatch: the build declares $declared, the archive holds $observed"
fi

stage="$version_dir/$platform"
/bin/rm -rf "$stage"
/bin/mkdir -p "$stage" || refuse "cannot create $stage"
# The tarball carries exactly one top-level `<product>-<version>/` directory,
# which is what stops an extraction from scattering node_modules across
# whatever directory tar happened to run in. Stripping it here is what makes
# the launcher land where the unit's program path says it is.
/usr/bin/tar -xzf "$inner" --strip-components=1 -C "$stage" \
  || refuse 'the web tarball did not unpack'
/bin/rm -rf "$release_dir"

[ -f "$stage/$launcher" ] || refuse "the web tarball carries no $launcher, so the unit would have nothing to run"
/bin/chmod u+x "$stage/$launcher" || refuse "cannot make $launcher executable"

# `current` is a symlink on every host this installs to, but a directory on
# hosts an older installer touched; either way the previous release is kept
# beside the new one rather than deleted, so a rollback is a relink.
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.before-$version" || refuse 'cannot retire the previous release directory'
else
  /bin/rm -f "$root/current"
fi
/bin/ln -sfn "$version_dir" "$root/.current.new" || refuse 'cannot stage the current link'
/bin/mv -f "$root/.current.new" "$root/current" || refuse 'cannot publish the current link'
printf 'STADO_WEB_INSTALL\t%s\n' "$version_dir"
"#;

/// The platform directory a release lands in on this host, by the same
/// shortening [`crate::deploy::service_catalog::resolve_word`] applies to
/// `$STADO_PLATFORM`. Derived through that function rather than restated, so
/// the directory the installer creates and the directory the unit's program
/// path names cannot drift apart.
fn platform_directory(target: &ComputeTarget) -> String {
    service_catalog::resolve_word(
        "$STADO_PLATFORM",
        "",
        Some(&target.release_platform),
        &target.name,
    )
}

/// The absolute program a web unit runs, with `$HOME` and `$STADO_PLATFORM`
/// resolved against the host that will run it.
pub(in crate::cli::web::deploy) fn launcher_program(
    target: &ComputeTarget,
    home: &str,
    product: &str,
) -> String {
    service_catalog::resolve_word(
        &format!(
            "$HOME/.stado/services/{product}/current/$STADO_PLATFORM/{}",
            LAUNCHER
        ),
        home,
        Some(&target.release_platform),
        &target.name,
    )
}

/// Install the release into the product's own service directory on the host.
pub(in crate::cli::web::deploy) async fn install_release(
    target: &ComputeTarget,
    product: &str,
    version: &str,
    archive_uri: &str,
    sha256: &str,
    runner: &Runner,
) -> Result<String, CmdError> {
    let script = format!(
        "set -eu\nname={}\nversion={}\nplatform={}\narchive_uri={}\nexpected={}\nlauncher={}\n{WEB_INSTALL_BODY}",
        crate::deploy::shlex_quote(product),
        crate::deploy::shlex_quote(version),
        crate::deploy::shlex_quote(&platform_directory(target)),
        crate::deploy::shlex_quote(archive_uri),
        crate::deploy::shlex_quote(sha256),
        crate::deploy::shlex_quote(LAUNCHER),
    );
    let output = host_channel::run_script_with_timeout(target, &script, INSTALL_TIMEOUT, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: could not install {product} {version}: {}",
            target.name,
            host_channel::last_error_line(&output, "the release did not install")
        )));
    }
    marker(&output.stdout, "STADO_WEB_INSTALL")
        .and_then(|fields| fields.first().copied())
        .filter(|directory| !directory.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: the installer reported no version directory for {product} {version}, so \
                 what `current` points at was never observed",
                target.name
            ))
        })
}
