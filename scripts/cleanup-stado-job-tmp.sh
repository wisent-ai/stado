#!/bin/sh
# Remove disposable /tmp state for explicitly named terminal Stado jobs.
# stado host run-helper only forwards UUIDs; the UUID's first group is the job id.
set -eu

[ "$#" -gt 0 ] || {
  printf '%s\n' 'at least one UUID carrying a Stado job id is required' >&2
  exit 2
}

is_active() {
  candidate=$1
  for link in /proc/[0-9]*/cwd /proc/[0-9]*/fd/*; do
    [ -L "$link" ] || continue
    target=$(readlink "$link" 2>/dev/null) || continue
    case "$target" in
      "$candidate"|"$candidate"/*) return 0 ;;
    esac
  done
  return 1
}

before=$(df -k /tmp | awk 'NR == 2 { print $4 }')
for correlation_id in "$@"; do
  job_id=${correlation_id%%-*}
  case "$job_id" in
    [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
    *)
      printf 'invalid Stado job id in UUID: %s\n' "$correlation_id" >&2
      exit 2
      ;;
  esac

  for candidate in \
    "/tmp/wc-$job_id" \
    "/tmp/echo-humanizer-$job_id" \
    "/tmp/oko-lifecycle-$job_id" \
    "/tmp/oko-lifecycle-model-$job_id" \
    "/tmp/jeden-goal-$job_id" \
    "/tmp/jeden-goal-model-$job_id"
  do
    [ -e "$candidate" ] || continue
    if is_active "$candidate"; then
      printf 'refusing active path: %s\n' "$candidate" >&2
      exit 1
    fi
    chmod -R u+w "$candidate" 2>/dev/null || true
    rm -rf "$candidate"
    printf 'removed=%s\n' "$candidate"
  done
done
after=$(df -k /tmp | awk 'NR == 2 { print $4 }')
printf 'free_kb_before=%s\nfree_kb_after=%s\nfreed_kb=%s\n' \
  "$before" "$after" "$((after - before))"
