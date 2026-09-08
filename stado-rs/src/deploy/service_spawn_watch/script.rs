//! The one fixed remote program the watch runs, and nothing else.

/// Read-only: `ps` on an interval, and nothing else. It starts nothing, stops
/// nothing, signals nothing, writes no file and needs no sudo.
///
/// `@MATCH@` is vetted by [`crate::deploy::service::quote_command_match`] — the same charset the
/// reaper's own filter allows, widened by exactly a space.
pub(super) const WATCH_SCRIPT: &str = "set -u
if [ \"$(/usr/bin/uname -s)\" != Darwin ]; then
  printf 'STADO_WATCH_UNSUPPORTED\\t%s\\n' \"$(/usr/bin/uname -s)\"
  exit 0
fi
match=@MATCH@
seconds=@SECONDS@
gap=@GAP@
self=$$
# This shell and every process above it are excluded by pid. The script itself
# arrives on stdin so the match is not in anybody's argv, but the sweep must
# still never report its own reader as an arrival.
mine=''
walk=$self
while [ -n \"$walk\" ] && [ \"$walk\" != 0 ] && [ \"$walk\" != 1 ]; do
  mine=\"$mine $walk\"
  walk=$(/bin/ps -p \"$walk\" -o ppid= 2>/dev/null | /usr/bin/tr -d ' ')
done
started=$(/bin/date +%s)
deadline=$(( started + seconds ))
known=''
first=yes
seq=0
samples=0
while :; do
  # ONE snapshot per sample. Every fact about this round -- who is new, who
  # its parent is, what that parent runs -- is read out of this one string,
  # because a second `ps` is a second moment and the parent may not be in it.
  snapshot=$(/bin/ps ax -o pid= -o ppid= -o lstart= -o command= 2>/dev/null)
  samples=$(( samples + 1 ))
  # The match rides the ENVIRONMENT, never argv: an `awk` invoked with
  # `stado agent` on its command line is itself a line containing
  # `stado agent`, and the sweep reported the searcher every single sample.
  hits=$(printf '%s\\n' \"$snapshot\" \
    | STADO_WATCH_MATCH=\"$match\" /usr/bin/awk 'index($0, ENVIRON[\"STADO_WATCH_MATCH\"]) > 0 { print $1 }')
  for pid in $hits; do
    case \" $mine \" in *\" $pid \"*) continue ;; esac
    case \" $known \" in *\" $pid \"*) continue ;; esac
    known=\"$known $pid\"
    row=$(printf '%s\\n' \"$snapshot\" \
      | /usr/bin/awk -v want=\"$pid\" '$1 == want { print; exit }' | /usr/bin/tr '\\t\\r\\n' ' ')
    if [ \"$first\" = yes ]; then
      printf 'STADO_WATCH_BASELINE\\t%s\\t%s\\n' \"$pid\" \"$row\"
      continue
    fi
    seq=$(( seq + 1 ))
    printf 'STADO_WATCH_ARRIVAL\\t%s\\t%s\\t%s\\t%s\\n' \\
      \"$seq\" \"$pid\" \"$(( $(/bin/date +%s) - started ))\" \"$row\"
    # The ancestry, out of the same snapshot, deepest-first from the arrival.
    # `alive` is asked of the live process table right now, so a report can
    # distinguish a parent that is still running from one already gone.
    up=$pid
    depth=0
    while [ -n \"$up\" ] && [ \"$up\" != 0 ] && [ \"$depth\" -lt 16 ]; do
      line=$(printf '%s\\n' \"$snapshot\" \
        | /usr/bin/awk -v want=\"$up\" '$1 == want { print; exit }' | /usr/bin/tr '\\t\\r\\n' ' ')
      if [ -z \"$line\" ]; then break; fi
      if /bin/ps -p \"$up\" -o pid= >/dev/null 2>&1; then alive=yes; else alive=no; fi
      printf 'STADO_WATCH_ANCESTOR\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$seq\" \"$depth\" \"$up\" \"$alive\" \"$line\"
      if [ \"$up\" = 1 ]; then break; fi
      up=$(printf '%s' \"$line\" | /usr/bin/awk '{ print $2 }')
      depth=$(( depth + 1 ))
    done
  done
  first=no
  if [ \"$(/bin/date +%s)\" -ge \"$deadline\" ]; then break; fi
  /bin/sleep \"$gap\"
done
printf 'STADO_WATCH_DONE\\t%s\\t%s\\n' \"$samples\" \"$(( $(/bin/date +%s) - started ))\"
";
