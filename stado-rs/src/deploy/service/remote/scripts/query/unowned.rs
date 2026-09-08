/// `service list --unowned`: product processes on one host that no unit owns.
///
/// Its own program rather than one more body on [`REMOTE_PRELUDE`], because it
/// addresses no unit: there is nothing to splice, and a sentinel unit id would
/// make the shared prelude derive a plist path for a unit that does not exist.
/// It is read-only in the strongest sense — it starts nothing, stops nothing,
/// signals nothing, and needs no sudo — so it is safe against a live host.
///
/// Two `stado agent` processes ran for four days on the always-on mac with no
/// launchd unit behind them, executing a binary older than the one on disk,
/// and every command in this group answered about declared units and so said
/// nothing at all about them. Ownership is asked of launchd itself: the pids
/// in the `services` table of each printable domain, and any descendant of one
/// of those pids, are owned. On Linux the same question is the cgroup the
/// kernel put the process in — a `.service` cgroup is a unit's, a `.scope` is
/// a login session's.
pub(crate) const UNOWNED_SCRIPT: &str = "set -u
os=$(/usr/bin/uname -s)
uid=$(/usr/bin/id -u)
set -- @ROOTS@
owned=''
owner_of=''
if [ \"$os\" = \"Darwin\" ]; then
  for launchd_domain in \"gui/$uid\" \"user/$uid\" system; do
    owned=\"$owned $(/bin/launchctl print \"$launchd_domain\" 2>/dev/null | /usr/bin/awk '/services = \\{/ { inside = 1; next } inside && /^[[:space:]]*\\}/ { inside = 0 } inside && $1 ~ /^[0-9]+$/ { print $1 }' | /usr/bin/tr '\\n' ' ')\"
  done
  # `owner_of` is set to the pid in the chain that matched, so a verdict of
  # \"owned\" can be checked instead of taken. The whole reason this command
  # answered an empty table for as long as it existed is that nothing printed
  # WHY a candidate was judged owned: launchd claims about a thousand pids on a
  # mac, and against a set that size the test is nearly always true.
  owns() {
    walk=\"$1\"
    owner_of=''
    while [ -n \"$walk\" ] && [ \"$walk\" != 0 ] && [ \"$walk\" != 1 ]; do
      case \" $owned \" in *\" $walk \"*) owner_of=\"$walk\"; return 0 ;; esac
      walk=$(/bin/ps -p \"$walk\" -o ppid= 2>/dev/null | /usr/bin/tr -d ' ')
    done
    return 1
  }
else
  # systemd hosts never build `owned`; the cgroup the kernel put the process in
  # is the whole answer. Counting `owned` unconditionally crashed every Linux
  # host with `owned: unbound variable` under `set -u`.
  owns() {
    cgroup=$(/bin/cat \"/proc/$1/cgroup\" 2>/dev/null | /usr/bin/sed -n 's/.*\\///p')
    owner_of=''
    case \"$cgroup\" in *.service) owner_of=\"$cgroup\"; return 0 ;; esac
    return 1
  }
fi
owned_count=0
for _pid in $owned; do owned_count=$((owned_count + 1)); done
printf 'STADO_UNOWNED_OWNED\\t%s\\n' \"$owned_count\"
seen=''
for root in \"$@\"; do
  matched=0
  under_count=0
  for pid in $(/usr/bin/pgrep -f \"$root\" 2>/dev/null); do
    matched=$((matched + 1))
    case \" $seen \" in *\" $pid \"*) continue ;; esac
    command=$(/bin/ps -p \"$pid\" -o command= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    if [ -z \"$command\" ]; then continue; fi
    exe=$(/bin/ps -p \"$pid\" -o comm= 2>/dev/null)
    entry=$(printf '%s' \"$command\" | /usr/bin/awk '{ print $2 }')
    # The root has to be what the process EXECUTES, not merely a word on its
    # command line: `pgrep -f` also matches a tail on a log under the root,
    # and a report that names those teaches operators to ignore it. An
    # interpreter is accepted on its entry point, which is the shape a
    # release tree runs under.
    under=no
    case \"$exe\" in \"$root\"*) under=yes ;; esac
    case \"$entry\" in \"$root\"*) under=yes ;; esac
    if [ \"$under\" = no ]; then continue; fi
    under_count=$((under_count + 1))
    if owns \"$pid\"; then
      # The verdict and its evidence, for every candidate. An operator reading
      # \"owned\" needs the pid in the ancestry that launchd actually claimed:
      # a chain that ends on a thousand-entry set is how 26 stado processes on
      # one host were all judged owned and none reported.
      printf 'STADO_UNOWNED_JUDGED\\t%s\\t%s\\t%s\\n' \"$pid\" 'owned' \"$owner_of\"
      continue
    fi
    printf 'STADO_UNOWNED_JUDGED\\t%s\\t%s\\t%s\\n' \"$pid\" 'unowned' '-'
    seen=\"$seen $pid\"
    started=$(/bin/ps -p \"$pid\" -o lstart= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    printf 'STADO_UNOWNED\\t%s\\t%s\\t%s\\n' \"$pid\" \"$started\" \"$command\"
  done
  # What this root actually searched, printed whether or not it found anything.
  # Without it an empty report is indistinguishable from a root that expanded
  # to a path no process could ever run out of, and the empty table was read as
  # \"no unowned processes\" for as long as this command has existed.
  printf 'STADO_UNOWNED_ROOT\\t%s\\t%s\\t%s\\n' \"$root\" \"$matched\" \"$under_count\"
done
";
