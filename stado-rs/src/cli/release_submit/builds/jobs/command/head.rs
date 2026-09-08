//! The bootstrap command's first half: the canonical job identity, the
//! persistent work root, and the guards each component of it must pass.

pub(super) const RELEASE_WORKER_COMMAND_HEAD: &str = r#"set -u
umask 077
case "${WC_JOB_ID:-}" in
  job-*) job_suffix=${WC_JOB_ID#job-} ;;
  *)
    printf '%s\n' '[release-worker-bootstrap] WC_JOB_ID is absent or noncanonical' >&2
    exit 1
    ;;
esac
if [ "${#job_suffix}" -ne 24 ]; then
  printf '%s\n' '[release-worker-bootstrap] WC_JOB_ID is absent or noncanonical' >&2
  exit 1
fi
case "$job_suffix" in
  *[!0123456789abcdef]*)
    printf '%s\n' '[release-worker-bootstrap] WC_JOB_ID is absent or noncanonical' >&2
    exit 1
    ;;
esac
old=$(/bin/pwd -P) || exit 1
home_root=$(CDPATH= cd "$HOME" && /bin/pwd -P) || exit 1
legacy_root=$(CDPATH= cd /tmp && /bin/pwd -P) || exit 1
stado_root="$home_root/.stado"
work_parent="$stado_root/work"
root="$work_parent/jobs"
work_name="wc-$WC_JOB_ID"
work="$root/$work_name"
legacy="$legacy_root/$work_name"
case "$old" in
  "$legacy"|"$work") ;;
  *)
    printf '%s\n' "[release-worker-bootstrap] refusing unexpected cwd: $old" >&2
    exit 1
    ;;
esac
job_pgid=$$
terminate_job_group() {
  trap '' TERM
  /bin/kill -TERM "-$job_pgid" 2>/dev/null || true
  /bin/sleep 2
  /bin/kill -KILL "-$job_pgid" 2>/dev/null || true
  exit 1
}
ensure_legacy_link() {
  if [ "$old" = "$work" ]; then
    return 0
  fi
  [ -d "$work" ] || return 1
  if [ -L "$old" ]; then
    [ "$(/usr/bin/readlink "$old")" = "$work" ]
    return
  fi
  if [ -e "$old" ]; then
    return 1
  fi
  /bin/ln -s "$work" "$old"
}
unlink_verified_legacy() {
  if [ -L "$old" ]; then
    [ "$(/usr/bin/readlink "$old")" = "$work" ] || return 1
    /bin/rm -f -- "$old"
    return
  fi
  [ ! -e "$old" ]
}
owner_uid=$(/usr/bin/id -u) || exit 1
owned_directory() {
  [ -d "$1" ] && [ ! -L "$1" ] || return 1
  [ "$(/usr/bin/find "$1" -prune -type d -user "$owner_uid" -print 2>/dev/null)" = "$1" ]
}
owned_regular_file() {
  [ -f "$1" ] && [ ! -L "$1" ] || return 1
  [ "$(/usr/bin/find "$1" -prune -type f -user "$owner_uid" -print 2>/dev/null)" = "$1" ]
}
prepare_component() {
  component="$1"
  expected="$2"
  if [ -L "$component" ]; then
    printf '%s\n' "[release-worker-bootstrap] refusing symlinked component: $expected" >&2
    return 1
  fi
  if [ ! -e "$component" ]; then
    /bin/mkdir "$component" || return 1
  fi
  [ -d "$component" ] && [ ! -L "$component" ]
}
"#;
