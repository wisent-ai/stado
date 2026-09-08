//! The prologue of the remote program: the mode bits every stage reads, and
//! the guards that decide whether a candidate may be taken at all.
//!
//! `reclaim` is here because it is the only place anything is removed, and
//! `held`, `process_absent`, `stale` and `stale_minutes` are here because
//! every stage below asks them rather than carrying its own answer.

/// `set -u` through `reclaim()`, the first segment of the remote program.
pub(super) const GUARDS: &str = r#"set -u
apply=@APPLY@
scratch="$HOME/@BUILD_WORK@"
services="$HOME/@SERVICES_ROOT@"
target_free_kb=@TARGET_FREE_KB@
keep_mode="@LOCAL_EVIDENCE_MODE@"
local_evidence="$HOME/@LOCAL_EVIDENCE_ROOT@"
local_grace=@LOCAL_TERMINALITY_GRACE_SECONDS@
stages=" @STAGES@ "

stage_enabled() {
  case "$stages" in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

free_kb() { /bin/df -Pk / 2>/dev/null | /usr/bin/awk 'NR==2 {print $4}'; }

# Every live process's argv, taken ONCE. Asking `ps` per candidate through a
# pipeline matches the grep's own argv and reports every path as held.
snapshot=$(/bin/ps -Ao args= 2>/dev/null || true)

held() {
  case "$snapshot" in
    *"$1"*) return 0 ;;
  esac
  return 1
}

# A queue job is launched with its workdir as cwd, inherited by the owning
# shell for the whole execution. The argv snapshot catches build children that
# name files inside it; lsof proves whether any process still has the tree as
# cwd or holds a file below it. Missing/failed lsof is unknown, never absent.
process_absent() {
  if held "$1"; then return 1; fi
  lsof_bin=""
  for candidate in /usr/sbin/lsof /usr/bin/lsof; do
    if [ -x "$candidate" ]; then lsof_bin="$candidate"; break; fi
  done
  [ -n "$lsof_bin" ] || return 2
  "$lsof_bin" -n +D "$1" >/dev/null 2>&1
  status=$?
  case "$status" in
    0) return 1 ;;
    1) return 0 ;;
    *) return 2 ;;
  esac
}

# Older than the age gate. Asked per candidate rather than by sweeping a root,
# so the candidates come from exactly one enumeration -- see below.
stale() {
  [ -n "$(/usr/bin/find "$1" -maxdepth 0 -mtime +@AGE_DAYS@ 2>/dev/null)" ]
}

stale_minutes() {
  [ -n "$(/usr/bin/find "$1" -maxdepth 0 -mmin +@CLONE_AGE_MINUTES@ 2>/dev/null)" ]
}

mtime_seconds() {
  /usr/bin/stat -f %m "$1" 2>/dev/null || /usr/bin/stat -c %Y "$1" 2>/dev/null
}

local_evidence() {
  entry="$1"
  id="$2"
  now=$(/bin/date +%s)
  tree_mtime=$(mtime_seconds "$entry") || {
    printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\tstat_failed\ttrue\tfalse\t0\t0\n' "$id" "$entry"
    return 1
  }
  tree_age=$((now - tree_mtime))
  process_absent "$entry"
  process_status=$?
  if [ "$process_status" -eq 1 ]; then
    printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\tprocess_present\tfalse\tfalse\t%s\t0\n' "$id" "$entry" "$tree_age"
    return 1
  fi
  if [ "$process_status" -ne 0 ]; then
    printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\tprocess_probe_unavailable\tfalse\tfalse\t%s\t0\n' "$id" "$entry" "$tree_age"
    return 1
  fi
  if [ "$tree_age" -lt "$local_grace" ]; then
    printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\ttree_too_young\ttrue\tfalse\t%s\t0\n' "$id" "$entry" "$tree_age"
    return 1
  fi
  evidence="$local_evidence/$id"
  observed_mtime=""
  absent_since=""
  if [ -r "$evidence" ]; then
    read -r observed_mtime absent_since < "$evidence" || true
  fi
  case "$observed_mtime:$absent_since" in
    "$tree_mtime":[0-9]*) ;;
    *)
      if [ "$apply" = 1 ]; then
        /bin/mkdir -p "$local_evidence" 2>/dev/null || {
          printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\tevidence_write_failed\ttrue\tfalse\t%s\t0\n' "$id" "$entry" "$tree_age"
          return 1
        }
        printf '%s %s\n' "$tree_mtime" "$now" > "$evidence.new" 2>/dev/null &&
          /bin/mv -f "$evidence.new" "$evidence" 2>/dev/null || {
            /bin/rm -f "$evidence.new" 2>/dev/null || true
            printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\tevidence_write_failed\ttrue\tfalse\t%s\t0\n' "$id" "$entry" "$tree_age"
            return 1
          }
      fi
      printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\tobservation_started\ttrue\tfalse\t%s\t0\n' "$id" "$entry" "$tree_age"
      return 1
      ;;
  esac
  absence_age=$((now - absent_since))
  if [ "$absence_age" -lt "$local_grace" ]; then
    printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\tlease_not_expired\ttrue\tfalse\t%s\t%s\n' "$id" "$entry" "$tree_age" "$absence_age"
    return 1
  fi
  if reclaim "$entry" queue_workdirs; then
    if [ "$apply" = 1 ]; then
      /bin/rm -f "$evidence" 2>/dev/null || true
      decision=reclaimed
    else
      decision=eligible
    fi
    printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\t%s\ttrue\ttrue\t%s\t%s\n' "$id" "$entry" "$decision" "$tree_age" "$absence_age"
    return 0
  fi
  printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\treclaim_refused\ttrue\ttrue\t%s\t%s\n' "$id" "$entry" "$tree_age" "$absence_age"
  return 1
}

# The only place anything is removed. A held path is skipped silently -- it is
# not a failure, it is the rule -- and in dry-run mode the path is reported
# without being touched, so a preview names exactly what an apply would take.
reclaim() {
  if held "$1"; then return 1; fi
  if [ "$apply" = 1 ]; then
    /bin/rm -rf -- "$1" 2>/dev/null || return 1
  fi
  printf 'STADO_RECLAIM_ITEM\t%s\t%s\n' "$2" "$1"
  return 0
}

"#;
