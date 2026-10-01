#!/bin/sh
# The pre-check runner is a role of the host's one Stado process.
#
# Drives the built `stado` binary through its real interface, with its state
# confined to the checkout's ignored build directory:
#   1. `stado serve --precheck-runner ROOT` with no launcher at ROOT refuses
#      by name and exits nonzero: the role's failure ends the process.
#   2. With a launcher present, the role starts it through passwordless sudo;
#      on a host that grants none the launcher cannot start, the process
#      reports that and exits nonzero, so a host without the grant never
#      looks like a host taking jobs.
#   3. `stado product catalog --json` lists every retired runner unit as a
#      role unit of `--precheck-runner`, and the driver's agents as retired
#      units of Probierz.
#
# Usage: tests/one-unit/precheck-runner-role.sh [path to stado binary]
set -eu

checkout=$(cd "$(dirname "$0")/../.." && pwd)
stado=${1:-$checkout/stado-rs/target/debug/stado}
# The build directory's name is short because the control socket lives in it
# and a Unix socket path is bounded by the kernel.
work=$checkout/stado-rs/target/ou
rm -rf "$work"
mkdir -p "$work/runner" "$work/home/.stado"
export HOME="$work/home"
export STADO_RELEASE_PROXY_SOCKET="$work/c.sock"
export WC_STORAGE_BACKEND=local
export WC_LOCAL_STORAGE_PATH="$work/home/.stado/local-storage"

fail() { printf 'precheck-runner-role: %s\n' "$1" >&2; exit 1; }

printf 'revision: %s\n' "$(git -C "$checkout" rev-parse HEAD)"
printf 'binary: %s\n' "$stado"

# 1. No launcher at ROOT: refused by name, process ends.
set +e
"$stado" serve --precheck-runner "$work/runner" >"$work/missing.out" 2>"$work/missing.err"
status=$?
set -e
printf 'serve without launcher: exit %s\n' "$status"
[ "$status" -ne 0 ] || fail 'serve exited 0 with no launcher to run'
grep -F "the pre-check runner launcher $work/runner/start-runner.sh does not exist" "$work/missing.err" >/dev/null ||
  fail "serve did not name the missing launcher: $(cat "$work/missing.err")"

# 2. A launcher is present: the role starts it under sudo -n. Without a
#    passwordless grant sudo refuses, the launcher is reported, the process
#    ends nonzero.
printf '#!/bin/sh\nexit 0\n' >"$work/runner/start-runner.sh"
chmod 755 "$work/runner/start-runner.sh"
set +e
"$stado" serve --precheck-runner "$work/runner" >"$work/launcher.out" 2>"$work/launcher.err"
status=$?
set -e
printf 'serve with launcher: exit %s\n' "$status"
[ "$status" -ne 0 ] || fail 'serve exited 0 although its runner role ended'
grep -F "[stado serve precheck-runner] $work/runner/start-runner.sh" "$work/launcher.err" >/dev/null ||
  fail "serve did not report starting the launcher: $(cat "$work/launcher.err")"
grep -F "the pre-check runner launcher $work/runner/start-runner.sh exited" "$work/launcher.err" >/dev/null ||
  fail "serve did not report the launcher's exit: $(cat "$work/launcher.err")"

# 3. The service catalog (`--output` writes the derived services document)
#    retires the runner units behind the role and the driver agents under
#    Probierz.
"$stado" product catalog --output "$work/catalog.json"
for unit in com.wisent.stado-precheck-runner wisent-stado-precheck-runner.service \
  com.wisent.stado-publisher-runner com.wisent.stado-product-precheck-runner; do
  grep -F "\"$unit\"" "$work/catalog.json" >/dev/null || fail "catalog does not retire $unit"
done
grep -F '"--precheck-runner"' "$work/catalog.json" >/dev/null || fail 'catalog names no --precheck-runner role'
grep -F '"com.wisent.probierz-cua-driver"' "$work/catalog.json" >/dev/null || fail 'catalog does not retire the driver agent'
printf 'catalog: runner units are role units of --precheck-runner; driver agents retired\n'
printf 'precheck-runner-role: passed\n'
