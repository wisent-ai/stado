//! The bootstrap command's second half: preparing the work root, running the
//! worker, and uploading and reading back its output at both coordinates.

pub(super) const RELEASE_WORKER_COMMAND_TAIL: &str = r#"cd "$home_root" || exit 1
[ "$(/bin/pwd -P)" = "$home_root" ] || exit 1
owned_directory . || exit 1
prepare_component .stado "$stado_root" || exit 1
cd .stado || exit 1
[ "$(/bin/pwd -P)" = "$stado_root" ] || exit 1
owned_directory . || exit 1
/bin/chmod 700 . || exit 1
prepare_component work "$work_parent" || exit 1
cd work || exit 1
[ "$(/bin/pwd -P)" = "$work_parent" ] || exit 1
owned_directory . || exit 1
/bin/chmod 700 . || exit 1
prepare_component jobs "$root" || exit 1
cd jobs || exit 1
if [ "$(/bin/pwd -P)" != "$root" ] || ! owned_directory .; then
  printf '%s\n' "[release-worker-bootstrap] persistent root changed: $root" >&2
  exit 1
fi
/bin/chmod 700 . || exit 1
if [ "$old" = "$legacy" ]; then
  if [ -e "$work_name" ] || [ -L "$work_name" ]; then
    printf '%s\n' "[release-worker-bootstrap] persistent workdir already exists: $work" >&2
    exit 1
  fi
  /bin/mv "$old" "$work_name" || exit 1
fi
if [ -L "$work_name" ] || ! owned_directory "$work_name"; then
  printf '%s\n' "[release-worker-bootstrap] unsafe persistent workdir: $work" >&2
  exit 1
fi
cd "$work_name" || exit 1
if [ "$(/bin/pwd -P)" != "$work" ] || ! owned_directory .; then
  printf '%s\n' "[release-worker-bootstrap] persistent workdir changed: $work" >&2
  exit 1
fi
/bin/chmod 700 . || exit 1
if [ "$old" = "$legacy" ]; then
  ensure_legacy_link || exit 1
fi
for child in output tmp; do
  prepare_component "$child" "$work/$child" || exit 1
  cd "$child" || exit 1
  if [ "$(/bin/pwd -P)" != "$work/$child" ] || ! owned_directory .; then
    printf '%s\n' "[release-worker-bootstrap] unsafe workdir child: $work/$child" >&2
    exit 1
  fi
  /bin/chmod 700 . || exit 1
  cd .. || exit 1
  [ "$(/bin/pwd -P)" = "$work" ] || exit 1
done
log_relative=output/command_output.log
log="$work/$log_relative"
owned_regular_file "$log_relative" || {
  printf '%s\n' "[release-worker-bootstrap] unsafe command log: $log" >&2
  exit 1
}
/bin/chmod 600 "$log_relative" || exit 1
TMPDIR="$work/tmp"
TMP="$TMPDIR"
TEMP="$TMPDIR"
export TMPDIR TMP TEMP
exec >>"$log_relative" 2>&1
attempt_output_uri=@RELEASE_OUTPUT_URI@
namespace_and_path=${attempt_output_uri#stado://}
queue_namespace=${namespace_and_path%%/*}
canonical_output_uri="stado://$queue_namespace/status/$WC_JOB_ID/output"
file_sha256() {
  digest=
  if [ -x /usr/bin/shasum ]; then
    set -- $(/usr/bin/shasum -a 256 "$1") || return 1
    digest="$1"
  elif [ -x /usr/bin/sha256sum ]; then
    set -- $(/usr/bin/sha256sum "$1") || return 1
    digest="$1"
  else
    return 1
  fi
  printf '%s' "$digest"
}
put_output_object() {
  scope="$1"
  base_uri="$2"
  leaf="$3"
  content_type="$4"
  source="$work/output/$leaf"
  owned_regular_file "$source" || return 1
  source_sha=$(file_sha256 "$source") || return 1
  set -- $(/usr/bin/wc -c <"$source") || return 1
  source_bytes="$1"
  owned_directory "$work/tmp" || return 1
  proof=$(/usr/bin/mktemp "$work/tmp/stado-upload.XXXXXX") || return 1
  owned_regular_file "$proof" || return 1
  /bin/chmod 600 "$proof" || return 1
  if ! "$HOME/.stado/bin/stado" storage put --content-type "$content_type" --json \
    "$base_uri/$leaf" "$source" >"$proof"; then
    /bin/rm -f -- "$proof"
    return 1
  fi
  if ! owned_regular_file "$proof" ||
    ! /usr/bin/grep -Eq '^[[:space:]]*"state":[[:space:]]*"(stored|replayed)",?[[:space:]]*$' "$proof" ||
    ! /usr/bin/grep -Eq "^[[:space:]]*\"sha256\":[[:space:]]*\"$source_sha\",?[[:space:]]*$" "$proof" ||
    ! /usr/bin/grep -Eq "^[[:space:]]*\"bytes\":[[:space:]]*$source_bytes,?[[:space:]]*$" "$proof"; then
    /bin/rm -f -- "$proof"
    return 1
  fi
  /bin/rm -f -- "$proof" || return 1
  printf '%s\n' "[release-worker-bootstrap] durable_output scope=$scope leaf=$leaf sha256=$source_sha bytes=$source_bytes"
}
# One store timeout must not discard a build that succeeded.
#
# The object store is a loopback service on the always-on host, so every
# builder reaches it across a tunnel or a relayed tailnet hop, and a single
# `storage put` that does not answer in time used to fail the whole release:
# on 2026-09-05 four coordinates were spent that way, each after
# `worker_exit_code=0` — the artifact was built and only its evidence upload
# timed out. Retry the exact same idempotent put; the store replays a byte
# identical object rather than storing a second one, so a retry cannot
# duplicate anything.
upload_output_object() {
  attempt=1
  wait_seconds=5
  while :; do
    if put_output_object "$@"; then
      return 0
    fi
    if [ "$attempt" -ge 4 ]; then
      printf '%s\n' "[release-worker-bootstrap] durable_output scope=$1 leaf=$3 failed after $attempt attempts" >&2
      return 1
    fi
    printf '%s\n' "[release-worker-bootstrap] durable_output scope=$1 leaf=$3 attempt $attempt did not settle; retrying in ${wait_seconds}s" >&2
    /bin/sleep "$wait_seconds"
    attempt=$((attempt + 1))
    wait_seconds=$((wait_seconds * 2))
  done
}
printf '%s\n' "[release-worker-bootstrap] workdir=$work tmpdir=$TMPDIR legacy_link=$old"
"$HOME/.stado/bin/stado" release worker --request release-request.json &
worker_pid=$!
while kill -0 "$worker_pid" 2>/dev/null; do
  if ! ensure_legacy_link; then
    printf '%s\n' "[release-worker-bootstrap] cannot preserve legacy link: $old" >&2
    terminate_job_group
  fi
  /bin/sleep 2
done
wait "$worker_pid"
rc=$?
evidence_upload_failed=0
if ! upload_output_object canonical "$canonical_output_uri" command_output.log text/plain; then
  evidence_upload_failed=1
fi
if ! upload_output_object attempt "$attempt_output_uri" command_output.log text/plain; then
  evidence_upload_failed=1
fi
if [ "$rc" -eq 0 ]; then
  if [ "$evidence_upload_failed" -eq 0 ]; then
    if ! upload_output_object canonical "$canonical_output_uri" release.tar.gz application/gzip; then
      evidence_upload_failed=1
    fi
    if ! upload_output_object attempt "$attempt_output_uri" release.tar.gz application/gzip; then
      evidence_upload_failed=1
    fi
  fi
  if [ "$evidence_upload_failed" -eq 0 ]; then
    if ! upload_output_object canonical "$canonical_output_uri" receipt.json application/json; then
      evidence_upload_failed=1
    fi
    if [ "$evidence_upload_failed" -eq 0 ] &&
      ! upload_output_object attempt "$attempt_output_uri" receipt.json application/json; then
      evidence_upload_failed=1
    fi
  fi
else
  if owned_regular_file "$work/output/receipt.json"; then
    if ! upload_output_object canonical "$canonical_output_uri" receipt.json application/json; then
      evidence_upload_failed=1
    fi
    if ! upload_output_object attempt "$attempt_output_uri" receipt.json application/json; then
      evidence_upload_failed=1
    fi
  fi
fi
if [ "$evidence_upload_failed" -ne 0 ]; then
  printf '%s\n' "[release-worker-bootstrap] output upload/read-back failed; worker_exit_code=$rc" >&2
  if [ "$rc" -eq 0 ]; then
    rc=1
  fi
fi
if ! ensure_legacy_link; then
  printf '%s\n' "[release-worker-bootstrap] cannot preserve legacy link: $old" >&2
  terminate_job_group
fi
if [ "$old" != "$work" ]; then
  (
    while [ -d "$work" ]; do
      if ! owned_directory "$work/tmp"; then
        printf '%s\n' "[release-worker-bootstrap] unsafe proof directory: $work/tmp" >&2
        break
      fi
      response=$(/usr/bin/mktemp "$work/tmp/stado-watch.XXXXXX") || {
        /bin/sleep 2
        continue
      }
      if ! owned_regular_file "$response" || ! /bin/chmod 600 "$response"; then
        /bin/rm -f -- "$response"
        /bin/sleep 2
        continue
      fi
      "$HOME/.stado/bin/stado" job watch "$WC_JOB_ID" --follow --json >"$response" &
      watch_pid=$!
      while kill -0 "$watch_pid" 2>/dev/null; do
        if [ ! -d "$work" ]; then
          /bin/kill -TERM "$watch_pid" 2>/dev/null || true
          break
        fi
        if ! ensure_legacy_link; then
          if [ ! -d "$work" ]; then
            /bin/kill -TERM "$watch_pid" 2>/dev/null || true
            break
          fi
          terminate_job_group
        fi
        /bin/sleep 2
      done
      wait "$watch_pid" 2>/dev/null || true
      if [ ! -d "$work" ]; then
        /bin/rm -f -- "$response"
        break
      fi
      if owned_regular_file "$response" &&
        /usr/bin/grep -Eq '^[[:space:]]*"terminal":[[:space:]]*true,?[[:space:]]*$' "$response"; then
        /bin/rm -f -- "$response"
        printf '%s\n' "[release-worker-bootstrap] lifecycle job_id=$WC_JOB_ID terminal=true"
        break
      fi
      /bin/rm -f -- "$response"
      if ! ensure_legacy_link; then
        [ ! -d "$work" ] && break
        terminate_job_group
      fi
      /bin/sleep 2
    done
    unlink_verified_legacy || exit 1
  ) </dev/null &
fi
exit "$rc"
"#;
