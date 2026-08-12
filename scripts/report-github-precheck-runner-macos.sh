#!/bin/sh
# Report macOS runner identity and executable trust without exposing credentials.
# Run through `stado host install-helper` + `run-helper`.
set -u

printf '%s\n' '== account =='
dscl . -read /Users/stado-precheck UniqueID PrimaryGroupID GeneratedUID NFSHomeDirectory UserShell AuthenticationAuthority Password 2>&1 || true
printf '%s\n' '== runner executables =='
for listener in \
  /Users/Shared/stado-precheck-runner/bin/Runner.Listener \
  "$HOME/.stado/actions-runner-brama/bin/Runner.Listener" \
  "$HOME/.stado/actions-runner-echo/bin/Runner.Listener"
do
  [ -f "$listener" ] || continue
  file "$listener"
  codesign --verify --verbose=2 "$listener" 2>&1 || true
done
printf '%s\n' '== platform protection =='
csrutil status 2>&1 || true
