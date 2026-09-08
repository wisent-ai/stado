use crate::deploy::service::*;

/// End every product process on TARGET that no DECLARED unit owns.
///
/// `service stop` ends a declared unit and the processes launchd disowned from
/// it. Nothing ended a product process whose label the registry never declared,
/// or whose label has since been removed while the process kept running — and
/// on charless-mac-mini that is how a `stado agent` from 2026-08-27 went on
/// publishing this host's capacity through three release deliveries, two
/// restarts, a `service stop` and a `service remove`, refusing 55 pinned jobs
/// the whole time.
///
/// Ownership here is the registry's, not launchd's. `unowned_processes` asks
/// whether ANY launchd job claims the process, and on a mac that set is about a
/// thousand pids, so a duplicate under an undeclared label reads as owned and
/// is left alone. This asks the question that matters: is this process the one
/// the document says should be running.
///
/// `SIGTERM` only, and only to processes executing out of a managed root whose
/// pid is not held by a declared label and is not a descendant of one. Nothing
/// is signalled on a `--dry-run`, which is the default at the CLI.
pub(crate) const REAP_SCRIPT: &str = "set -u
if [ \"$(/usr/bin/uname -s)\" != Darwin ]; then
  printf 'STADO_REAP_UNSUPPORTED\\t%s\\n' \"$(/usr/bin/uname -s)\"
  exit 0
fi
apply=@APPLY@
match=@MATCH@
set -- @ROOTS@
# The pids the DECLARED labels hold, and their descendants. Everything else
# under a managed root is a process the document does not account for.
#
# `launchctl list` prints only the domain this login can print, so a declared
# SYSTEM LaunchDaemon's pid was never in this set and every process it owns read
# as unowned. On charless-mac-mini on 2026-09-01 that made a fleet-wide
# `reap --command 'stado agent'` propose ending pid 3963 -- the queue agent
# `service ensure` had just installed as `com.wisent.compute.service.stado-agent-mini`
# in the system domain, thirty seconds earlier, and the only DECLARED agent the
# host had. Its argv is byte-identical to the undeclared duplicate beside it, so
# no `--command` substring could separate them and the operator's only options
# were to end the declared agent too or not to reap at all.
#
# So the keep-set asks launchd for the label when the listing does not have it.
# `launchctl print <domain>/<label>` reads the system domain without privilege
# and states the `pid` the job holds; only the `pid` line is taken. This is the
# same blindness the loaded-label scan had against `com.wisent.*` and the same
# remedy: ask the host about the whole world, not about the part one command
# happens to print.
keep=''
uid=$(/usr/bin/id -u)
listing=$(/bin/launchctl list)
for label in @LABELS@; do
  pid=$(printf '%s\\n' \"$listing\" | /usr/bin/awk -F'\\t' -v l=\"$label\" '$3 == l && $1 ~ /^[0-9]+$/ { print $1 }')
  if [ -z \"$pid\" ]; then
    for domain in system \"user/$uid\" \"gui/$uid\"; do
      pid=$(/bin/launchctl print \"$domain/$label\" 2>/dev/null |
        /usr/bin/awk -F' = ' '$1 ~ /^[[:space:]]*pid$/ { print $2; exit }' |
        /usr/bin/tr -d ' ')
      case \"$pid\" in
        ''|*[!0-9]*) pid='' ;;
        *) break ;;
      esac
    done
  fi
  if [ -n \"$pid\" ]; then keep=\"$keep $pid\"; fi
done
kept() {
  walk=\"$1\"
  while [ -n \"$walk\" ] && [ \"$walk\" != 0 ] && [ \"$walk\" != 1 ]; do
    case \" $keep \" in *\" $walk \"*) return 0 ;; esac
    walk=$(/bin/ps -p \"$walk\" -o ppid= 2>/dev/null | /usr/bin/tr -d ' ')
  done
  return 1
}
printf 'STADO_REAP_KEEP\\t%s\\n' \"$(printf '%s' \"$keep\" | /usr/bin/tr -s ' ')\"
self=$$
seen=''
for root in \"$@\"; do
  for pid in $(/usr/bin/pgrep -f \"$root\" 2>/dev/null); do
    case \" $seen \" in *\" $pid \"*) continue ;; esac
    if [ \"$pid\" = \"$self\" ]; then continue; fi
    command=$(/bin/ps -p \"$pid\" -o command= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    if [ -z \"$command\" ]; then continue; fi
    exe=$(/bin/ps -p \"$pid\" -o comm= 2>/dev/null)
    entry=$(printf '%s' \"$command\" | /usr/bin/awk '{ print $2 }')
    under=no
    case \"$exe\" in \"$root\"*) under=yes ;; esac
    case \"$entry\" in \"$root\"*) under=yes ;; esac
    if [ \"$under\" = no ]; then continue; fi
    # The operator names the exact program being de-duplicated. Without this
    # the keep-set decides the blast radius, and launchd holds a pid for only
    # some declared labels: a fleet-wide dry run on charless-mac-mini proposed
    # ending `skarbiec serve`, `stado dashboard`, `stado resolver serve` and the
    # Weles API server, every one of them a live service, because their pids are
    # not the ones their labels hold. One named program cannot do that.
    case \"$command\" in *\"$match\"*) ;; *) continue ;; esac
    seen=\"$seen $pid\"
    started=$(/bin/ps -p \"$pid\" -o lstart= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    # A kept pid is never signalled, and it used to be dropped here - before
    # its command was ever printed. That hid the one process an operator most
    # needs to name: the program a DECLARED label is holding, which is where a
    # stale binary survives a delivery. On charless-mac-mini the writer
    # starving the janitor's interval was pid 78635 under
    # `com.wisent.compute.service.stado-local-control-plane`, and every reap
    # report could say only its number. Reporting is not signalling: the row
    # reads `kept` and the loop still refuses to touch it.
    if kept \"$pid\"; then
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'kept' \"$started\" \"$command\"
      continue
    fi
    if [ \"$apply\" != yes ]; then
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'would_end' \"$started\" \"$command\"
      continue
    fi
    /bin/kill \"$pid\" 2>/dev/null || true
    /bin/sleep 2
    if /bin/ps -p \"$pid\" -o pid= >/dev/null 2>&1; then
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'still_running' \"$started\" \"$command\"
    else
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'ended' \"$started\" \"$command\"
    fi
  done
done
";

/// A command substring that can ride inside double quotes in the fixed remote
/// program.
///
/// [`quote_unit_path`] refuses a space, which is right for a unit path and
/// wrong here: the whole point of the filter is to name
/// `stado agent --target <host>` rather than a bare binary. The charset is
/// widened by exactly a space and nothing else, so every character a shell
/// would act on stays refused. Shared with
/// [`crate::deploy::service_spawn_watch`], which filters the same process table for
/// the same kind of name and must refuse exactly what the reaper refuses.
pub fn quote_command_match(value: &str) -> Result<String, DeployError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(DeployError("the command substring is empty".to_string()));
    }
    let allowed = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(c, ' ' | '-' | '_' | '.' | '/' | '=' | ':' | '+' | ',')
    };
    if let Some(bad) = trimmed.chars().find(|c| !allowed(*c)) {
        return Err(DeployError(format!(
            "command substring {trimmed:?} contains {bad:?}, which cannot ride the fixed remote \
             program"
        )));
    }
    Ok(trimmed.to_string())
}
