//! The macOS runner runtime: verifying upstream apphosts, maintaining stable
//! local signing identities, and reading a publisher runner's state.

pub(crate) const MACOS_RUNTIME_FUNCTIONS: &str = r#"
runner_signatures_valid() {
  root /usr/bin/codesign --verify --strict -R '=anchor apple generic' "$runner_root/bin/Runner.Listener" >/dev/null 2>&1 &&
  root /usr/bin/codesign --verify --strict -R '=anchor apple generic' "$runner_root/bin/Runner.Worker" >/dev/null 2>&1
}
resolve_runner_release() {
  version=$(jq -er '.libraries | keys | map(select(startswith("Runner.Listener/"))) | if length == 1 then .[0] | ltrimstr("Runner.Listener/") else error("ambiguous runner version") end' "$runner_root/bin/Runner.Listener.deps.json")
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { printf '%s\n' 'runner version is malformed' >&2; exit 1; }
  curl --fail --silent --show-error --location --max-time 60 \
    "https://api.github.com/repos/actions/runner/releases/tags/v$version" -o "$staging/release.json"
  expected=$(jq -er --arg name "actions-runner-osx-arm64-$version.tar.gz" \
    '.assets | map(select(.name == $name)) | if length == 1 then .[0].digest | strings | select(startswith("sha256:")) | ltrimstr("sha256:") else error("runner artifact is ambiguous") end' "$staging/release.json")
  [[ "$expected" =~ ^[0-9a-f]{64}$ ]] || { printf '%s\n' 'release has no SHA-256 digest' >&2; exit 1; }
}
fetch_runner_archive() {
  curl --fail --silent --show-error --location --max-time 120 \
    "https://github.com/actions/runner/releases/download/v$version/actions-runner-osx-arm64-$version.tar.gz" \
    -o "$archive"
  actual=$(shasum -a 256 "$archive" | cut -d' ' -f1)
  [ "$actual" = "$expected" ] || { printf '%s\n' "runner checksum mismatch: $actual" >&2; exit 1; }
}
restore_runner_apphosts() {
  signed_runtime="$staging/signed-runtime"
  mkdir -p "$signed_runtime"
  tar -xzf "$archive" -C "$signed_runtime" ./bin/Runner.Listener ./bin/Runner.Worker
  for executable in Runner.Listener Runner.Worker; do
    /usr/bin/codesign --verify --strict "$signed_runtime/bin/$executable"
  done
  "${WISENT_PRODUCTS_BIN:-$HOME/.local/bin/wisent-products}" signing sign --product stado \
    "$signed_runtime/bin/Runner.Listener" "$signed_runtime/bin/Runner.Worker"
  for executable in Runner.Worker Runner.Listener; do
    owner=$(stat -f '%u:%g' "$runner_root/bin/$executable")
    replacement=$(root mktemp "$runner_root/bin/.$executable.stado.XXXXXX")
    root cp "$signed_runtime/bin/$executable" "$replacement"
    root chown "$owner" "$replacement"
    root chmod 0755 "$replacement"
    root mv -f "$replacement" "$runner_root/bin/$executable"
  done
}
"#;

pub(crate) const MACOS_RUNTIME_REPAIR: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_root=__RUNNER_ROOT__
__MACOS_RUNTIME_FUNCTIONS__
[ -f "$runner_root/.runner" ] || { printf '%s\n' 'runner is not registered' >&2; exit 1; }
if runner_signatures_valid; then
  printf '%s\n' 'runner apphost signatures are intact; no files changed'
  exit 0
fi
mkdir -p "$HOME/.stado/work"
staging=$(mktemp -d "$HOME/.stado/work/runner-runtime.XXXXXX")
trap 'root rm -rf "$staging"' EXIT HUP INT TERM
archive="$staging/runner.tar.gz"
resolve_runner_release
fetch_runner_archive
restore_runner_apphosts
runner_signatures_valid
printf 'restored runner %s with stable Apple identities; upstream archive sha256=%s; no service restarted\n' "$version" "$expected"
"#;

pub(crate) const MACOS_PUBLISHER_STATUS: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_root=/Users/Shared/stado-precheck-runner
runner_user=stado-precheck
if ! root launchctl print system/com.wisent.stado-precheck-runner >/dev/null; then
  root plutil -lint /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist >&2 || true
  root tail -n 80 "$runner_root/_diag/launchd.stderr.log" >&2 || true
  exit 1
fi
dscl . -read "/Users/$runner_user" UniqueID PrimaryGroupID NFSHomeDirectory UserShell Password >/dev/null
root pfctl -a com.wisent.stado-precheck -sr >/dev/null
identity_output=$(root sudo -u "$runner_user" -H -- /usr/bin/security find-identity -v -p codesigning "$runner_root/Library/Keychains/login.keychain-db" 2>&1 || true)
printf '%s\n' "$identity_output" | grep -F '"Developer ID Application:' >/dev/null
scope=$(root sed -n '3p' "$runner_root/.stado/registered-runner" 2>/dev/null || true)
newest_log=$(root sh -c "ls -t \"$runner_root\"/_diag/Runner_*.log 2>/dev/null | head -n 1")
if [ -n "$newest_log" ]; then
  listener_state=$(root tail -n 400 "$newest_log" | /usr/bin/grep -a -E 'Listening for Jobs|Running job|Job .* completed|Terminate|Error|Exception' | /usr/bin/tail -n 1 || true)
  [ -n "$listener_state" ] || listener_state='no listener event in the last 400 log lines'
else
  listener_state='no runner diagnostic log, so the listener has never started'
fi
job_holder=none
for marker in /Users/Shared/.stado-runner-jobs/*.job; do
  [ -f "$marker" ] || continue
  pid=$(root head -n 1 "$marker" 2>/dev/null || true)
  case "$pid" in ''|*[!0-9]*) continue ;; esac
  if kill -0 "$pid" 2>/dev/null; then job_holder="$(basename "$marker" .job) pid=$pid"; else job_holder="$(basename "$marker" .job) stale"; fi
done
printf 'listener: %s\nhost job slot: %s\n%s\n' "$listener_state" "$job_holder" "${scope:-unrecorded}"
"#;
