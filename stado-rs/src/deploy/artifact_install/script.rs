//! The remote program, in the three pieces an install concatenates: the
//! single-file body, the archive body, and the sweep that runs after either
//! one has already reported success.

/// The script that does the work on the host.
///
/// Written so that a failure at any step leaves the previous `current` intact:
/// the download lands in the version directory, the digest is checked there,
/// and only a verified file causes the symlink to move. `ln -sfn` through a
/// temporary name makes the final swap atomic, so a reader never observes a
/// `current` that points at nothing.
pub(super) const INSTALL_BODY: &str = r#"
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
previous_dir="$(cd "$root" 2>/dev/null && readlink current 2>/dev/null || true)"
ln -sfn "$version_dir" "$root/.current.new"
mv -f "$root/.current.new" "$root/current"
echo "STADO_STATUS=installed"
echo "STADO_DETAIL=$program"
"#;

/// Retire the version trees this install just made unreachable.
///
/// Every install stages one tree per version beside the one before it, and
/// nothing ever removed the ones a rollback can no longer reach. Measured on
/// `charless-mac-mini` on 2026-09-05: superseded trees under
/// `~/.stado/services` for `stado-object-api` and `weles-admission` held about
/// 6 GiB while the disk sat at 6.1 GiB against the janitor's 15 GiB low
/// watermark, so the host claimed nothing and the space came back only because
/// an operator ran `stado space reclaim` by hand - four times in one day. The
/// janitor cannot reach these: its declared cleaners cover the release store,
/// build caches, queue workdirs and browser clones, and a delivered service
/// tree is none of those.
///
/// What a rollback or an operator can still reach is what stays: the tree
/// `current` now points at, the tree it pointed at before this install
/// (`stado service update --rollback-to` is a relink onto it), every
/// operator-made `current.*` backup, and anything modified within the last
/// day, which covers a delivery still in flight. The sweep runs after the
/// install has already reported success and cannot fail it.
pub(super) const PRUNE_BODY: &str = r#"
(
  set +e
  for entry in "$root"/*; do
    [ -d "$entry" ] || continue
    case "${entry##*/}" in current|current.*|.*) continue ;; esac
    [ "$entry" = "$version_dir" ] && continue
    [ -n "${previous_dir:-}" ] && [ "$entry" = "$previous_dir" ] && continue
    [ -n "$(find "$entry" -maxdepth 0 -mtime +1 2>/dev/null)" ] || continue
    rm -rf "$entry" && printf 'STADO_RETIRED_TREE=%s\n' "$entry"
  done
) || true
"#;

pub(super) const INSTALL_ARCHIVE_BODY: &str = r#"
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
previous_dir="$(cd "$root" 2>/dev/null && readlink current 2>/dev/null || true)"
ln -sfn "$version_dir" "$root/.current.new"
mv -f "$root/.current.new" "$root/current"
echo "STADO_STATUS=installed"
echo "STADO_DETAIL=$dest"
"#;
