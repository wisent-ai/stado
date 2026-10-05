use crate::deploy::service::*;

/// End every product process on TARGET that no DECLARED unit owns.
///
/// `service stop` ends a declared unit and the processes launchd disowned from
/// it. Nothing ended a product process whose label the registry never declared,
/// or whose label has since been removed while the process kept running — so
/// a stale `stado agent` could go on publishing a host's capacity through
/// release deliveries, restarts, a `service stop` and a `service remove`,
/// refusing pinned jobs the whole time.
///
/// Ownership here is the registry's, not launchd's. `unowned_processes` asks
/// whether ANY launchd job claims the process, and on a mac that set is about a
/// thousand pids, so a duplicate under an undeclared label reads as owned and
/// is left alone. This asks the question that matters: is this process the one
/// the document says should be running.
///
/// Only processes executing out of a managed root, or whose working directory
/// is under one, whose pid is not held by a declared unit and is not a
/// descendant of one. On Linux a process in the cgroup of a declared systemd
/// unit is held by it, whatever its parent. Nothing is signalled on a dry run,
/// which is the default at the CLI; `--apply` sends SIGKILL.
pub(crate) const REAP_SCRIPT: &str = "set -u
os=$(/usr/bin/uname -s)
apply=@APPLY@
match=@MATCH@
set -- @ROOTS@
# The pids the DECLARED labels hold, and their descendants. Everything else
# under a managed root is a process the document does not account for.
#
# `launchctl list` prints only the domain this login can print, so a declared
# SYSTEM LaunchDaemon's pid would never be in this set and every process it
# owns would read as unowned: a fleet-wide `reap --command 'stado agent'` would
# propose ending the queue agent `service ensure` had just installed in the
# system domain, the only DECLARED agent the host has. Its argv is
# byte-identical to an undeclared duplicate beside it, so no `--command`
# substring could separate them and the operator's only options would be to
# end the declared agent too or not to reap at all.
#
# So the keep-set asks launchd for the label when the listing does not have it.
# `launchctl print <domain>/<label>` reads the system domain without privilege
# and states the `pid` the job holds; only the `pid` line is taken. This is the
# same blindness the loaded-label scan had against `com.wisent.*` and the same
# remedy: ask the host about the whole world, not about the part one command
# happens to print.
keep=''
uid=$(/usr/bin/id -u)
if [ \"$os\" = Darwin ]; then
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
else
  # A declared systemd unit, system or user, holds its main pid.
  for label in @LABELS@; do
    for scope in --system --user; do
      pid=$(/usr/bin/systemctl \"$scope\" show -p MainPID --value \"$label\" 2>/dev/null)
      case \"$pid\" in ''|0|*[!0-9]*) ;; *) keep=\"$keep $pid\" ;; esac
    done
  done
fi
# On Linux the kernel names the unit a process belongs to: a declared unit's
# cgroup holds every process it started, including one a double fork left
# without a parent, which no ancestry walk can reach.
declared_cgroup() {
  if [ \"$os\" = Darwin ]; then return 1; fi
  leaf=$(/usr/bin/awk -F/ '$NF ~ /\\.service$/ { print $NF; exit }' \"/proc/$1/cgroup\" 2>/dev/null)
  for label in @LABELS@; do
    case \"$leaf\" in \"$label\"|\"$label.service\") return 0 ;; esac
  done
  return 1
}
kept() {
  if declared_cgroup \"$1\"; then return 0; fi
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
  printf 'STADO_REAP_ROOT\\t%s\\n' \"$root\"
  pids=$(/usr/bin/pgrep -f \"$root\" 2>&1)
  scan_code=$?
  flat_pids=$(printf '%s' \"$pids\" | /usr/bin/tr '\\r\\n' ' ')
  printf 'STADO_REAP_SCAN\\t%s\\t%s\\t%s\\n' \"$root\" \"$scan_code\" \"$flat_pids\"
  if [ \"$scan_code\" -gt 1 ]; then exit \"$scan_code\"; fi
  # A process started inside the root with relative paths (`bash
  # release/build.sh`, `node node_modules/...`) names no path under it on its
  # command line; its working directory does.
  if [ \"$os\" = Darwin ]; then
    cwd_pids=$(/usr/sbin/lsof -nP -a -d cwd -u \"$uid\" -Fpn 2>/dev/null | /usr/bin/awk -v root=\"$root\" '/^p/ { pid = substr($0, 2) } /^n/ { if (index(substr($0, 2), root \"/\") == 1) print pid }' | /usr/bin/tr '\\n' ' ')
  else
    cwd_pids=''
    for proc in /proc/[0-9]*; do
      case \"$(/bin/readlink \"$proc/cwd\" 2>/dev/null)\" in \"$root\"/*) cwd_pids=\"$cwd_pids ${proc#/proc/}\" ;; esac
    done
  fi
  for pid in $pids $cwd_pids; do
    case \" $seen \" in *\" $pid \"*) continue ;; esac
    if [ \"$pid\" = \"$self\" ]; then continue; fi
    command=$(/bin/ps -ww -p \"$pid\" -o command= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    printf 'STADO_REAP_EXAMINED\\t%s\\t%s\\t%s\\n' \"$pid\" \"$root\" \"$command\"
    if [ -z \"$command\" ]; then continue; fi
    exe=$(/bin/ps -ww -p \"$pid\" -o comm= 2>/dev/null)
    entry=$(printf '%s' \"$command\" | /usr/bin/awk '{ print $2 }')
    under=no
    # macOS comm can be truncated; command retains the launched argv path.
    case \"$command\" in \"$root\"/*) under=yes ;; esac
    case \"$exe\" in \"$root\"/*) under=yes ;; esac
    case \"$entry\" in \"$root\"/*) under=yes ;; esac
    case \" $cwd_pids \" in *\" $pid \"*) under=yes ;; esac
    if [ \"$under\" = no ]; then continue; fi
    # The operator names the exact program being de-duplicated. Without this
    # the keep-set decides the blast radius, and launchd holds a pid for only
    # some declared labels: a fleet-wide dry run would propose ending
    # `skarbiec serve`, `stado serve` and the Weles API server, every one of
    # them a live service, because their pids
    # are not the ones their labels hold. One named program cannot do that.
    case \"$command\" in *\"$match\"*) ;; *) continue ;; esac
    seen=\"$seen $pid\"
    started=$(/bin/ps -p \"$pid\" -o lstart= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    # A kept pid is never signalled, and dropping it here - before its command
    # was ever printed - would hide the one process an operator most needs to
    # name: the program a DECLARED label is holding, which is where a stale
    # binary survives a delivery and can starve the janitor's interval while
    # every reap report says only its number. Reporting is not signalling: the
    # row reads `kept` and the loop still refuses to touch it.
    if kept \"$pid\"; then
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'kept' \"$started\" \"$command\"
      continue
    fi
    if [ \"$apply\" != yes ]; then
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'would_end' \"$started\" \"$command\"
      continue
    fi
    # A duplicate is ended with a signal it cannot ignore, and the report is
    # written when the kernel says the process exited: kqueue through
    # caffeinate on macOS, the pid watch of GNU tail on Linux.
    if /bin/kill -KILL \"$pid\" 2>/dev/null; then
      if [ -x /usr/bin/caffeinate ]; then
        /usr/bin/caffeinate -w \"$pid\"
      else
        tail --pid=\"$pid\" -f /dev/null
      fi
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'ended' \"$started\" \"$command\"
    else
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'still_running' \"$started\" \"$command\"
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
