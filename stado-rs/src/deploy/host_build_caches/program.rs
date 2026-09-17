//! The fixed remote program, plus the marker prefix, the tag signature and
//! the environment contract it reads.

/// Marker prefix of the remote script's report lines.
pub const STATUS_PREFIX: &str = "STADO_BUILD_CACHE\t";

/// The standard's required first line, which the remote script greps for.
pub const CACHEDIR_SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

/// Environment contract of the remote script.
pub const ROOT_ENV: &str = "STADO_CACHE_ROOT";
pub const AGE_ENV: &str = "STADO_CACHE_MIN_AGE_DAYS";
pub const APPLY_ENV: &str = "STADO_CACHE_APPLY";
pub const FORCE_ENV: &str = "STADO_CACHE_FORCE";
/// Newline-separated home-relative directories the walk never opens. The
/// value is the janitor's own refused-root list for the target's platform
/// (`privacy_protected_parts`), so the verdict and the janitor refuse the
/// same doors.
pub const PRUNE_ENV: &str = "STADO_CACHE_PRUNE";

/// Two phases. First one `find` pass collects the tag files, so the walk is
/// never mutated underneath itself — deleting during the walk made `find`
/// fail and swallowed the report while the deletions still happened. Then
/// each owning directory is judged by its own mtime and, when applying,
/// removed. A cache nested inside a cache needs no special case: its parent
/// is reported and removed whole.
///
/// The walk prunes every root named in `STADO_CACHE_PRUNE` before opening
/// it, and reads `find`'s own error lines instead of its exit status: a
/// directory it could not open is one `permission-denied` row and the walk
/// goes on. `scan-failed` is kept for the case where `find` failed and
/// produced no tag at all.
pub const REMOTE_SCRIPT: &str = r#"root="${STADO_CACHE_ROOT:-}"
days="${STADO_CACHE_MIN_AGE_DAYS:-}"
apply="${STADO_CACHE_APPLY:-}"
force="${STADO_CACHE_FORCE:-}"
signature='Signature: 8a477f597d28d172789f06886806bc55'
snapshot=$(/bin/ps -Ao args= 2>/dev/null || true)
lsof_bin=""
for candidate in /usr/sbin/lsof /usr/bin/lsof; do
  if [ -x "$candidate" ]; then lsof_bin="$candidate"; break; fi
done

process_absent() {
  case "$snapshot" in
    *"$1"*) return 1 ;;
  esac
  [ -n "$lsof_bin" ] || return 2
  # Build processes normally keep their cwd at the project root, not inside
  # `target`. Looking only below the tagged cache returned "idle" while Cargo
  # was still writing it, and `rm` then raced those writes. The tag's parent is
  # the narrow owner boundary that covers both cwd and cache files without
  # treating an unrelated process elsewhere under the operator's scan root as
  # a holder.
  owner=$(/usr/bin/dirname "$1")
  "$lsof_bin" -n +D "$owner" >/dev/null 2>&1
  status=$?
  case "$status" in
    0) return 1 ;;
    1) return 0 ;;
    *) return 2 ;;
  esac
}

if [ -z "$root" ] || [ ! -d "$root" ]; then
  printf 'STADO_BUILD_CACHE\troot-absent\t%s\t%s\n' "$root" -
  exit
fi

prune="${STADO_CACHE_PRUNE:-}"
set --
if [ -n "$prune" ]; then
  saved_ifs=$IFS
  IFS='
'
  for part in $prune; do
    [ -n "$part" ] || continue
    if [ $# -eq 0 ]; then
      set -- -path "$HOME/$part"
    else
      set -- "$@" -o -path "$HOME/$part"
    fi
  done
  IFS=$saved_ifs
fi
if [ $# -gt 0 ]; then
  listing=$(/usr/bin/find "$root" \( "$@" \) -prune -o -type f -name CACHEDIR.TAG -print 2>&1)
else
  listing=$(/usr/bin/find "$root" -type f -name CACHEDIR.TAG -print 2>&1)
fi
find_status=$?
tags=""
other_errors=""
while IFS= read -r line; do
  [ -n "$line" ] || continue
  case "$line" in
    find:\ *)
      case "$line" in
        *": Permission denied"|*": Operation not permitted")
          dir=${line#find: }
          dir=${dir%: Permission denied}
          dir=${dir%: Operation not permitted}
          printf 'STADO_BUILD_CACHE\tpermission-denied\t%s\t%s\n' "$dir" -
          ;;
        *)
          other_errors="$other_errors$line
"
          ;;
      esac
      ;;
    *)
      tags="$tags$line
"
      ;;
  esac
done <<STADO_LISTING
$listing
STADO_LISTING
if [ -z "$tags" ] && [ "$find_status" -ne 0 ] && [ -n "$other_errors" ]; then
  printf '%s' "$other_errors" >&2
  printf 'STADO_BUILD_CACHE\tscan-failed\t%s\t%s\n' "$root" -
  exit 1
fi
if [ -n "$other_errors" ]; then
  printf '%s' "$other_errors" >&2
fi
if [ -z "$tags" ]; then
  printf 'STADO_BUILD_CACHE\tno-cache-tags\t%s\t%s\n' "$root" -
  exit 0
fi

printf '%s\n' "$tags" |
(
failed=0
while IFS= read -r tag; do
  [ -n "$tag" ] || continue
  dir=$(/usr/bin/dirname "$tag")
  case "$dir" in
    "$root")
      printf 'STADO_BUILD_CACHE\troot-protected\t%s\t%s\n' "$dir" -
      continue
      ;;
  esac
  if ! /usr/bin/grep -qxF "$signature" "$tag" 2>/dev/null; then
    printf 'STADO_BUILD_CACHE\tuntagged\t%s\t%s\n' "$dir" -
    continue
  fi
  if [ -n "$force" ]; then
    aged="$dir"
  else
    aged=$(/usr/bin/find "$dir" -maxdepth 0 -mtime "+$days" 2>/dev/null || true)
  fi
  if [ -z "$aged" ]; then
    printf 'STADO_BUILD_CACHE\ttoo-young\t%s\t%s\n' "$dir" -
    continue
  fi
  size=$(/usr/bin/du -sk "$dir" 2>/dev/null | /usr/bin/awk '{print $1}')
  process_absent "$dir"
  process_status=$?
  if [ "$process_status" -eq 1 ]; then
    printf 'STADO_BUILD_CACHE\tin-use\t%s\t%s\n' "$dir" "${size:--}"
    continue
  fi
  if [ "$process_status" -ne 0 ]; then
    printf 'STADO_BUILD_CACHE\tliveness-unavailable\t%s\t%s\n' "$dir" "${size:--}"
    continue
  fi
  if [ -n "$apply" ]; then
    if /bin/rm -rf "$dir"; then
      printf 'STADO_BUILD_CACHE\tremoved\t%s\t%s\n' "$dir" "${size:--}"
    else
      printf 'STADO_BUILD_CACHE\tremove-failed\t%s\t%s\n' "$dir" "${size:--}"
      failed=1
    fi
  else
    printf 'STADO_BUILD_CACHE\tcandidate\t%s\t%s\n' "$dir" "${size:--}"
  fi
done
exit "$failed"
)
"#;
