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

/// Two phases. First one `find` pass collects the tag files, so the walk is
/// never mutated underneath itself — deleting during the walk made `find`
/// fail and swallowed the report while the deletions still happened. Then
/// each owning directory is judged by its own mtime and, when applying,
/// removed. A cache nested inside a cache needs no special case: its parent
/// is reported and removed whole.
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

if ! tags=$(/usr/bin/find "$root" -type f -name CACHEDIR.TAG); then
  printf 'STADO_BUILD_CACHE\tscan-failed\t%s\t%s\n' "$root" -
  exit 1
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
