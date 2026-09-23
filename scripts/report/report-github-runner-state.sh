#!/bin/sh
# Report the local GitHub Actions runner without changing its launchd state.
# Run through `stado host install-helper` + `run-helper`; it accepts no input.
set -eu

PATH="/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

printf 'now: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf '\n== matching launchd jobs ==\n'
labels="$(launchctl list 2>/dev/null | awk 'tolower($3) ~ /(actions|runner|github)/ { print $3 }')"
if [ -z "$labels" ]; then
  printf '  (none)\n'
else
  printf '%s\n' "$labels" | sed 's/^/  /'
  for label in $labels; do
    printf '\n== launchd %s ==\n' "$label"
    launchctl print "gui/$(id -u)/$label" 2>&1 | sed -n '1,80p' || true
  done
fi

printf '\n== matching processes ==\n'
ps ax -o user= -o pid= -o ppid= -o etime= -o command= 2>/dev/null \
  | grep -iE '[R]unner\.(Listener|Worker)|[r]unsvc\.sh|[a]ctions-runner|[g]ithub-runner' \
  | sed 's/^/  /' || printf '  (none)\n'

printf '\n== latest runner diagnostic ==\n'
found=false
for directory in "$HOME/actions-runner/_diag" \
                 "$HOME/.github-runner/_diag" \
                 "$HOME/github-runner/_diag" \
                 "$HOME/.stado/github-runner/_diag" \
                 "/Users/Shared/stado-precheck-runner/_diag" \
                 "/Users/Shared/jeden-desktop-release-runner/_diag"; do
  [ -d "$directory" ] || continue
  latest=
  for log in "$directory"/Runner_*.log; do
    [ -f "$log" ] && latest="$log"
  done
  [ -n "$latest" ] || continue
  found=true
  printf '  %s\n' "$latest"
  tail -n 40 "$latest" | sed 's/^/  /'
done
$found || printf '  (no known diagnostic directory)\n'
