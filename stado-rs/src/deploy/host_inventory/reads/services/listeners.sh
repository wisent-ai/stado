# The kernel socket table, nothing else. No lsof, no pgrep -f, no /proc walk:
# the owner is reported as a bare pid, and mapping that pid to a program is
# `stado host exec TARGET -- ps ax -o pid -o ppid -o etime -o comm`, which is
# already approved and already argument-free.
#
# Collected first and judged after, because an empty socket table makes every
# marker on the host look stale, and that is a fleet-wide incident report.
# "netstat did not answer" has to be a state of its own rather than an empty
# list passed off as "nothing is listening".
listeners_state=read
listeners_json=""
if ! netstat_raw=$(/usr/sbin/netstat -anv -p tcp 2>/dev/null); then
  listeners_state=failed
  netstat_raw=""
fi
if [ "$listeners_state" = read ]; then
  if ! listeners_json=$(printf '%s\n' "$netstat_raw" | /usr/bin/awk '
  $6 != "LISTEN" { next }
  {
    address = $4
    parts_count = split(address, parts, ".")
    port = parts[parts_count]
    if (port !~ /^[0-9]+$/) next
    host = substr(address, 1, length(address) - length(port) - 1)
    # EVERY listening socket, once per address:port, and no interface filter.
    # This used to drop anything that was not loopback and then keep only the
    # FIRST row per port, which made two facts unrepresentable: that a port has
    # more than one holder, and that anything is listening on a routable
    # address at all. charless-mac-mini had THREE servers on 8765 — the
    # declared unit on 127.0.0.1, a stale duplicate on ::1, and an undeclared
    # node proxy on the tailnet address serving every external caller — and
    # this table could show exactly one of them. Consumers that only care
    # about loopback filter for it themselves; a de-duplicating collector
    # cannot be un-de-duplicated downstream.
    if (seen[address]++) next
    pid = 0
    for (field = 7; field <= NF; field++) {
      if ($field ~ /:[0-9]+$/) {
        pid = substr($field, index($field, ":") + 1)
        break
      }
    }
    printf "%s{\"address\":\"%s\",\"port\":%d,\"pid\":%d}", (emitted++ ? "," : ""), host, port + 0, pid + 0
  }
'); then
    listeners_state=failed
    listeners_json=""
  fi
fi
printf '],"listeners":[%s],"listeners_state":"%s","subcommands":[' \
  "$listeners_json" "$listeners_state"

