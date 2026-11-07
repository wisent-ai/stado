//! The second look of the remote program: the record of a candidate an
//! earlier apply found unheld, the modification time it is judged by, the
//! newest-of choice every keep-the-newest stage makes, and the queue work
//! trees' local evidence.

/// `one_line`, `settled`, `newest_by_age` and `local_evidence`, the segment
/// after the guards and the shared `mtime_seconds`
/// (`host_inventory/reads/filesystem/mtime.sh`).
pub(super) const SETTLED: &str = r#"
# TEXT on one line, so a refusal that carries an answer of the operating
# system stays one tab-separated record of four fields.
one_line() { printf '%s' "$1" | /usr/bin/tr '\t\n' '  '; }

# A path no process holds, found so by an earlier apply and unchanged since.
# The first apply that finds it unheld records its modification time and
# keeps it; a later apply takes it only when no process holds it then either
# and nothing has been added to or removed from it in between. Age is not the
# evidence: a tree can be old and in use, or young and abandoned. Answers 0
# when the stage may take it. A dry run records nothing, so it names exactly
# what the next apply would take. A modification time that cannot be read, or
# a record that cannot be written or reads back other than written, keeps the
# candidate and says why: a second look that never lands would keep every
# candidate for good without a word.
settled() {
  # A declared tree is never a candidate, so it records no first look and a
  # preview names the declaration that keeps it.
  if declared_named "$1"; then
    printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" "a registry service declaration names $declared_by; retained"
    return 1
  fi
  process_absent "$1"
  case $? in
    0) ;;
    1) return 1 ;;
    *)
      printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" 'process ownership could not be read; retained'
      return 1
      ;;
  esac
  mtime_seconds "$1" || {
    printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" "its modification time cannot be read ($(one_line "$mtime_read")); retained: if it was removed meanwhile, the next apply no longer lists it; otherwise make /usr/bin/stat runnable or repair the volume the cause names, then apply again"
    return 1
  }
  settled_mtime=$mtime_read
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
    settled_written=$( { /bin/mkdir -p "$local_evidence/settled" && printf '%s\n' "$settled_mtime" > "$settled_record"; } 2>&1 ) || {
      printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" "the second-look record $settled_record cannot be written ($(one_line "$settled_written")); retained: clear what the cause names (a full or read-only volume, or $local_evidence/settled not a directory this account can write), then apply again"
      return 1
    }
    settled_back=$(/bin/cat -- "$settled_record" 2>&1) || true
    if [ "$settled_back" != "$settled_mtime" ]; then
      printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" "the second-look record $settled_record reads back '$(one_line "$settled_back")', not '$settled_mtime'; retained: remove $settled_record, then apply again"
      return 1
    fi
  fi
  printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$2" "$1" 'unheld on this look; the next apply takes it if no process holds it and it is unchanged'
  return 1
}

# The newest of the given entries that PREDICATE accepts, by modification
# time, left in $newest. Every keep-the-newest stage chooses with this one
# function. An accepted entry whose age cannot be read leaves the newest
# unknown: the stage is refused naming that entry and the operating system's
# answer, and the caller takes nothing, because leaving the entry out would
# shield an older one instead.
newest_by_age() {
  newest_stage="$1"
  newest_predicate="$2"
  shift 2
  newest=""
  newest_age=""
  for newest_candidate in "$@"; do
    "$newest_predicate" "$newest_candidate" || continue
    mtime_seconds "$newest_candidate" || {
      printf 'STADO_RECLAIM_REFUSED\t%s\t%s\t%s\n' "$newest_stage" "$newest_candidate" "its age cannot be read ($(one_line "$mtime_read")), so the newest entry is unknown and none is taken: if it was removed meanwhile, the next apply no longer lists it; otherwise make /usr/bin/stat runnable or repair the volume the cause names, then apply again"
      return 1
    }
    if [ -z "$newest" ] || [ "$mtime_read" -gt "$newest_age" ]; then
      newest="$newest_candidate"
      newest_age="$mtime_read"
    fi
  done
}

local_evidence() {
  entry="$1"
  id="$2"
  now=$(/bin/date +%s)
  mtime_seconds "$entry" || {
    printf 'STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\t%s\t%s\tstat_failed\ttrue\tfalse\t0\t0\n' "$id" "$entry"
    printf 'STADO_RECLAIM_REFUSED\tqueue_workdirs\t%s\t%s\n' "$entry" "its modification time cannot be read ($(one_line "$mtime_read")); retained: if it was removed meanwhile, the next apply no longer lists it; otherwise make /usr/bin/stat runnable or repair the volume the cause names, then apply again"
    return 1
  }
  tree_mtime=$mtime_read
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
"#;
