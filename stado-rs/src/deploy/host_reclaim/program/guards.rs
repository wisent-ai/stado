//! The prologue of the remote program: the mode bits every stage reads, and
//! the guards that decide whether a candidate may be taken at all.
//!
//! `reclaim` is here because it is the only place anything is removed, and
//! `held`, `unit_named`, `declared_named` and `process_absent` are here
//! because every stage below asks them rather than carrying its own answer.
//! The second look (`settled`) and the modification time it reads are the
//! next segment, [`super::settled`].

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
# The kernel's name, read once; every stat spelling and platform gate
# follows it.
platform=$(/usr/bin/uname -s 2>/dev/null || /bin/uname -s)

stage_enabled() {
  case "$stages" in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

# The volume the stages reclaim from: everything they remove lives under
# $HOME, and `/` is the sealed system snapshot on macOS.
free_kb() { /bin/df -Pk "$HOME" 2>/dev/null | /usr/bin/awk 'NR==2 {print $4}'; }

# Every live process's argv, taken ONCE. Asking `ps` per candidate through a
# pipeline matches the grep's own argv and reports every path as held. A
# table that cannot be read is not an empty one: ownership is then unknown
# for every path, and `process_absent` answers so.
if snapshot=$(/bin/ps -Ao args= 2>&1); then
  snapshot_read=yes
else
  snapshot_read=no
fi

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
  if [ "$snapshot_read" != yes ]; then return 2; fi
  if held "$1"; then return 1; fi
  lsof_holds "$1"
  case $? in
    0) return 1 ;;
    1) return 0 ;;
    *) return 2 ;;
  esac
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

# A path a registry service declaration on this host names -- its program,
# its unit file, an absolute argument -- or one that holds such a path, or
# one such a path holds. The registry is what the fleet will start, restart
# and converge from, so a tree it names is kept even when no unit file and no
# process names it now: taking it would leave the declaration naming nothing.
# The paths are read from the registry by the caller before this program is
# built; a registry it cannot read refuses the whole reclamation there.
declared_named() {
  for declared in @DECLARED_PATHS@; do
    case "$declared" in
      "$1"|"$1"/*) declared_by="$declared"; return 0 ;;
    esac
    case "$1" in
      "$declared"/*) declared_by="$declared"; return 0 ;;
    esac
  done
  return 1
}

# The only place anything is removed. A held or unit-named path is skipped
# silently -- it is not a failure, it is the rule. A path a registry service
# declaration names is kept and reported with the declared path, so a
# preview says why a tree that looks superseded stays. A path whose unit
# scopes could not be read is kept and reported as refused, since absence was
# never proven. In dry-run mode the path is reported without being touched,
# so a preview names exactly what an apply would take.
reclaim() {
  if [ "$snapshot_read" != yes ]; then
    printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" "the process table could not be read ($(printf '%s' "$snapshot" | /usr/bin/tr '\t\n' '  ')); retained"
    return 1
  fi
  if held "$1"; then return 1; fi
  if declared_named "$1"; then
    printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" "a registry service declaration names $declared_by; retained"
    return 1
  fi
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
    if ! removal=$(/bin/rm -rf -- "$1" 2>&1); then
      printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" "removal failed: $(printf '%s' "$removal" | /usr/bin/tr '\t\n' '  ')"
      return 1
    fi
  fi
  printf 'STADO_RECLAIM_ITEM\t%s\t%s\n' "$2" "$1"
  return 0
}

"#;
