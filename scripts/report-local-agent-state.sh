#!/bin/sh
# Report the local Stado agent and its recent journal without changing service state.
# Run through `stado host install-helper` + `run-helper`; it accepts no input.
set -eu

PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

printf 'now: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf '\n== matching systemd units ==\n'
units="$(systemctl list-units --all --no-legend --no-pager 2>/dev/null \
  | awk 'tolower($1) ~ /stado|agent/ { print $1 }')"
if [ -z "$units" ]; then
  printf '  (none)\n'
else
  printf '%s\n' "$units" | sed 's/^/  /'
  for unit in $units; do
    printf '\n== %s ==\n' "$unit"
    systemctl status "$unit" --no-pager -n 0 2>&1 | sed -n '1,16p' || true
    systemctl show "$unit" --no-pager \
      -p FragmentPath -p DropInPaths -p ExecStart -p EnvironmentFiles 2>/dev/null \
      | sed 's/^/  /' || true
    journalctl -u "$unit" -n 80 --no-pager 2>&1 | sed 's/^/  /' || true
  done
fi

printf '\n== matching processes ==\n'
ps ax -o user= -o pid= -o ppid= -o etime= -o command= 2>/dev/null \
  | grep -iE '[s]tado( |$).*agent|[s]tado-local-agent' \
  | sed 's/^/  /' || printf '  (none)\n'

for pid in $(pgrep -f '[.]stado/bin/stado agent --target' 2>/dev/null || true); do
  printf '\n== process %s ==\n' "$pid"
  printf 'exe: %s\n' "$(readlink "/proc/$pid/exe" 2>/dev/null || printf unavailable)"
  printf 'cwd: %s\n' "$(readlink "/proc/$pid/cwd" 2>/dev/null || printf unavailable)"
  sed -n -E '/^(Name|State|Pid|PPid|Threads|VmRSS|VmSize):/p' "/proc/$pid/status" 2>/dev/null || true
  printf 'cgroup:\n'
  sed 's/^/  /' "/proc/$pid/cgroup" 2>/dev/null || true
  for fd in 1 2; do
    destination="$(readlink "/proc/$pid/fd/$fd" 2>/dev/null || printf unavailable)"
    printf 'fd%s: %s\n' "$fd" "$destination"
    [ -f "$destination" ] && tail -n 40 "$destination" | sed 's/^/  /' || true
  done
done
