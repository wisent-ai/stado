//! The prologue of the remote program: the mode bits every stage reads, and
//! the guards that decide whether a candidate may be taken at all.
//!
//! `reclaim` is here because it is the only place anything is removed, and
//! `held`, `unit_named`, `process_absent` and `settled` are here because
//! every stage below asks them rather than carrying its own answer.

/// `lsof_holds PATH`: 0 when a process has PATH (or, for a directory,
/// anything below it) open or as its working directory, 1 when none does, 2
/// when that cannot be read. Shared by every remote program that removes a
/// tree, so they cannot disagree about what holding means.
///
/// lsof answers 1 whenever any file it was asked about is not open — with
/// `+D` that is any file below the tree, held or not — so its listing, not
/// its status, is the answer. Deciding by the status read every non-empty
/// held tree as free.
pub(crate) const LSOF_HOLDS: &str = r#"lsof_holds() {
  lsof_bin=""
  for candidate in /usr/sbin/lsof /usr/bin/lsof; do
    if [ -x "$candidate" ]; then lsof_bin="$candidate"; break; fi
  done
  [ -n "$lsof_bin" ] || return 2
  if [ -d "$1" ]; then
    lsof_listing=$("$lsof_bin" -n +D "$1" 2>/dev/null)
  else
    lsof_listing=$("$lsof_bin" -n -- "$1" 2>/dev/null)
  fi
  lsof_status=$?
  [ -n "$lsof_listing" ] && return 0
  [ "$lsof_status" -le 1 ] && return 1
  return 2
}
"#;

/// `set -u` through `reclaim()`, the first segment of the remote program.
pub(super) const GUARDS: &str = r#"set -u
apply=@APPLY@
scratch="$HOME/@BUILD_WORK@"
services="$HOME/@SERVICES_ROOT@"
keep_mode="@LOCAL_EVIDENCE_MODE@"
local_evidence="$HOME/@LOCAL_EVIDENCE_ROOT@"
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
# name files inside it; `lsof_holds` proves whether any process still has the
# tree as cwd or holds a file below it. Unknown is never absent.
process_absent() {
  if held "$1"; then return 1; fi
  lsof_holds "$1"
  case $? in
    0) return 1 ;;
    1) return 0 ;;
    *) return 2 ;;
  esac
}

# A path no process holds, found so by an earlier apply and unchanged since.
# The first apply that finds it unheld records its modification time and
# keeps it; a later apply takes it only when no process holds it then either
# and nothing has been added to or removed from it in between. Age is not the
# evidence: a tree can be old and in use, or young and abandoned. Answers 0
# when the stage may take it. A dry run records nothing, so it names exactly
# what the next apply would take.
settled() {
  process_absent "$1"
  case $? in
    0) ;;
    1) return 1 ;;
    *)
      printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" 'process ownership could not be read; retained'
      return 1
      ;;
  esac
  settled_mtime=$(mtime_seconds "$1") || return 1
  settled_key=$(printf '%s' "$1" | /usr/bin/cksum | /usr/bin/cut -d' ' -f1)
  settled_record="$local_evidence/settled/$2-$settled_key"
  settled_seen=""
  if [ -r "$settled_record" ]; then
    read -r settled_seen < "$settled_record" || true
  fi
  if [ "$settled_seen" = "$settled_mtime" ]; then
    if [ "$apply" = 1 ]; then
      /bin/rm -f "$settled_record" 2>/dev/null || true
    fi
    return 0
  fi
  if [ "$apply" = 1 ]; then
    /bin/mkdir -p "$local_evidence/settled" 2>/dev/null &&
      printf '%s\n' "$settled_mtime" > "$settled_record" 2>/dev/null
  fi
  printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" 'unheld on this look; the next apply takes it if no process holds it and it is unchanged'
  return 1
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

# A path a unit definition on this host names is an installed service program,
# not scratch, wherever it lives: the unit runs it again on its next start,
# so it is kept while the service is down or crash-looping and no process
# names it. Units are read once per candidate from every init scope. Answers
# 0 named, 1 named by no unit, 2 when a unit scope could not be read: grep's
# own error status, so an unreadable definition never reads as absence.
unit_named() {
  for units in @UNIT_DIRECTORIES@; do
    [ -d "$units" ] || continue
    /usr/bin/grep -rqsF -- "$1" "$units"
    case $? in
      0) return 0 ;;
      1) ;;
      *) return 2 ;;
    esac
  done
  return 1
}

# The only place anything is removed. A held or unit-named path is skipped
# silently -- it is not a failure, it is the rule. A path whose unit scopes
# could not be read is kept and reported as refused, since absence was never
# proven. In dry-run mode the path is reported without being touched, so a
# preview names exactly what an apply would take.
reclaim() {
  if held "$1"; then return 1; fi
  unit_named "$1"
  case $? in
    1) ;;
    0) return 1 ;;
    *)
      printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" 'unit definitions could not be read; retained'
      return 1
      ;;
  esac
  if [ "$apply" = 1 ]; then
    /bin/rm -rf -- "$1" 2>/dev/null || return 1
  fi
  printf 'STADO_RECLAIM_ITEM\t%s\t%s\n' "$2" "$1"
  return 0
}

"#;
