//! What macOS reads back, restarts and removes for one installed runner.

/// Each check states its own refusal, because the caller reports the last
/// error line and this script tails the runner's launchd logs: for as long as
/// `set -e` alone decided the outcome, a failing check surfaced as `precheck
/// runner status failed: No ALTQ support in kernel` — a pfctl warning from a
/// log written hours earlier, about a step that had nothing to do with the
/// one that failed.
pub(crate) const MACOS_STATUS: &str = r#"set -uo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_root=/Users/Shared/stado-precheck-runner
fail() { printf '__RUNNER_KIND__ runner: %s\n' "$1" >&2; exit 1; }
if ! root launchctl print system/com.wisent.stado-precheck-runner >/dev/null 2>&1; then
  root plutil -lint /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist >&2 || true
  root tail -n 80 "$runner_root/_diag/launchd.stderr.log" >&2 || true
  fail 'launchd daemon com.wisent.stado-precheck-runner is not loaded'
fi
root launchctl print system/com.wisent.stado-precheck-runner |
  grep -F 'state = running' >/dev/null ||
  fail 'launchd daemon com.wisent.stado-precheck-runner is loaded but not running'
dscl . -read /Users/stado-precheck UniqueID PrimaryGroupID NFSHomeDirectory UserShell >/dev/null 2>&1 ||
  fail 'service account stado-precheck does not exist'
dscl . -read /Users/stado-precheck Password >/dev/null 2>&1 ||
  fail 'service account stado-precheck has no Password attribute, so it is not the locked account the installer creates'
root pfctl -a com.wisent.stado-precheck -sr >/dev/null 2>&1 ||
  fail 'pf anchor com.wisent.stado-precheck carries no ruleset, so private-network egress is not blocked'
# The listener that IS running, not one this check starts.
#
# This used to re-execute `Runner.Listener --version` under `sudo -u`, and on
# a host whose runner was executing jobs at that moment the probe answered
# `Failed to create CoreCLR, HRESULT: 0x8007000C`: a single-file .NET bundle
# started from an ad-hoc sudo context has no launchd domain of its own, so the
# check measured its own invocation rather than the product. The runner is a
# system daemon, so what can be observed about it is the process launchd keeps
# and the account that owns it.
# No `root` here: the process table is world-readable, and the host's
# passwordless sudo is granted for named commands only — asking for `ps`
# through it is refused, which is a fact about sudoers rather than about the
# runner.
# The whole table is consumed rather than cut short: `awk ... { exit }` closes
# the pipe while `ps` is still writing, `ps` dies of SIGPIPE, and `pipefail`
# then reports "the process table could not be read" on a host where reading
# it worked perfectly.
# Scoped to THIS runner's root. Matching any `Runner.Listener` passed on the
# wrong process: `charless-mac-mini` runs four of them, one of which
# (`/Users/Shared/jeden-desktop-release-runner`) happens to run as the same
# account, so the check reported a healthy listener while the pre-check
# runner's own listener had been dead since 18:51:45 and every job for these
# labels queued.
listener_owner=$(/bin/ps -Ao user=,comm= |
  /usr/bin/awk -v root="$runner_root/" \
    '$2 ~ /Runner\.Listener$/ && index($2, root) == 1 && !seen++ { owner = $1 } END { print owner }') ||
  fail 'the process table could not be read'
listener_problem=
if [ -z "$listener_owner" ]; then
  daemon_out=$(root tail -n 3 "$runner_root/_diag/launchd.stdout.log" 2>/dev/null | /usr/bin/tr '\n' ' ')
  daemon_err=$(root tail -n 3 "$runner_root/_diag/launchd.stderr.log" 2>/dev/null | /usr/bin/tr '\n' ' ')
  runner_log=$(root sh -c "ls -t \"$runner_root\"/_diag/Runner_*.log 2>/dev/null | head -n 1")
  runner_tail=$(root tail -n 3 "$runner_log" 2>/dev/null | /usr/bin/tr '\n' ' ')
  owned=$(/bin/ps -Ao user= | /usr/bin/grep -c -x 'stado-precheck')
  listener_problem="no listener is running from $runner_root, so this host takes no jobs for its labels. stado-precheck holds $owned processes. wrapper stdout: $daemon_out | wrapper stderr: $daemon_err | runner log ($runner_log): $runner_tail"
elif [ "$listener_owner" != "stado-precheck" ]; then
  listener_problem="the runner listener runs as $listener_owner, not stado-precheck"
fi
agent_id=$(root cat "$runner_root/routes/kronika-agent-id" 2>/dev/null) ||
  fail 'routes/kronika-agent-id is unreadable'
[ -n "$agent_id" ] || fail 'routes/kronika-agent-id is empty'
brama_route=$(root cat "$runner_root/routes/brama.url" 2>/dev/null) ||
  fail 'routes/brama.url is unreadable'
[ -n "$brama_route" ] || fail 'routes/brama.url is empty'
secret_meta=$(root stat -f '%Su %Sg %Lp' "$runner_root/.stado/kronika-agent-auth-secret" 2>/dev/null) ||
  fail 'the kronika agent signing secret is missing'
[ "$secret_meta" = "stado-precheck stado-precheck 600" ] ||
  fail "the kronika agent signing secret is $secret_meta, not stado-precheck stado-precheck 600"
# Whether the listener is CONNECTED, which is a different fact from whether a
# process exists. A registration the runner can no longer use leaves the
# process up and every job for this label queued forever; the only place that
# shows is the runner's own diagnostic log, and nothing in this product read
# it. On 2026-09-06 a Skarbiec documentation gate sat queued for half an hour
# against a host whose daemon was `state = running`.
newest_log=$(root sh -c "ls -t \"$runner_root\"/_diag/Runner_*.log 2>/dev/null | head -n 1")
if [ -n "$newest_log" ]; then
  listener_state=$(root tail -n 400 "$newest_log" |
    /usr/bin/grep -a -E 'Listening for Jobs|Running job|Job .* completed|Terminate|Error|Exception' |
    /usr/bin/tail -n 1)
  [ -n "$listener_state" ] || listener_state='no listener event in the last 400 log lines'
else
  listener_state='no runner diagnostic log, so the listener has never started'
fi
if [ -n "$listener_problem" ]; then listener_state=$listener_problem; fi
# Every runner listener this host runs, with its owner and its path. A host
# carries several runners, and "a Runner.Listener is running" says nothing
# about which one: the reclaim phase of `restart` needs the process that
# belongs to THIS runner root, and an operator reading a queued job needs the
# same distinction.
listeners=$(/bin/ps -Ao user=,comm= |
  /usr/bin/awk '$2 ~ /Runner\.Listener$/ { printf "%s %s; ", $1, $2 }')
scope=$(root sed -n '3p' "$runner_root/.stado/registered-runner" 2>/dev/null || true)
job_holder=none
for marker in /Users/Shared/.stado-runner-jobs/*.job; do
  [ -f "$marker" ] || continue
  pid=$(root head -n 1 "$marker" 2>/dev/null || true)
  case "$pid" in ''|*[!0-9]*) continue ;; esac
  if kill -0 "$pid" 2>/dev/null; then job_holder="$(basename "$marker" .job) pid=$pid"; else job_holder="$(basename "$marker" .job) stale"; fi
done
printf 'kronika agent: %s\nbrama route: %s\nkronika signing secret: owner=%s\nlistener: %s\nrunner listeners: %s\nhost job slot: %s\n%s\n' "$agent_id" "$brama_route" "$secret_meta" "$listener_state" "${listeners:-none}" "$job_holder" "${scope:-unrecorded}"
"#;

/// Restart the runner in place and wait until it says it is listening again.
///
/// A listener whose long poll to GitHub's broker is cut keeps its process and
/// its `state = running`, and takes no jobs: `install` sees a running service
/// and leaves it alone, so nothing in this product could recover it. On
/// 2026-09-06 a reinstall cut that session at 18:51:45 —
/// `[ERR BrokerServer] System.Net.Sockets.SocketException (89): Operation
/// canceled` — and a Skarbiec documentation gate then sat queued for half an
/// hour against a host that looked healthy in every other reading.
///
/// `kickstart -k` replaces the job without a window in which it does not
/// exist, and the wait is on the runner's own log rather than on the daemon
/// state that was never the question.
pub(crate) const MACOS_RESTART: &str = r#"set -uo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_root=/Users/Shared/stado-precheck-runner
fail() { printf '__RUNNER_KIND__ runner: %s\n' "$1" >&2; exit 1; }
newest_log_path() {
  root sh -c "ls -t \"$runner_root\"/_diag/Runner_*.log 2>/dev/null | head -n 1"
}
# The evidence has to be NEWER than the restart. A whole-file `grep
# 'Listening for Jobs'` matches the line the runner wrote when it first
# started, so the first version of this wait reported success on a listener
# that had not reconnected at all — the same "the check I happened to run"
# failure this repository keeps paying for.
await_listening() {
  before_path=$1
  before_bytes=$2
  deadline=$(( $(date +%s) + $3 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    sleep 5
    current=$(newest_log_path)
    [ -n "$current" ] || continue
    if [ "$current" != "$before_path" ]; then
      # The runner rotated to a log of its own, so anything in it is new.
      fresh=$(root cat "$current" 2>/dev/null)
    else
      fresh=$(root tail -c "+$(( before_bytes + 1 ))" "$current" 2>/dev/null)
    fi
    if printf '%s' "$fresh" | /usr/bin/grep -a -q 'Listening for Jobs'; then
      return 0
    fi
  done
  return 1
}
snapshot() {
  snapshot_path=$(newest_log_path)
  snapshot_bytes=0
  if [ -n "$snapshot_path" ]; then
    snapshot_bytes=$(root stat -f %z "$snapshot_path" 2>/dev/null || printf '0')
  fi
}

snapshot
root launchctl kickstart -k system/com.wisent.stado-precheck-runner ||
  fail 'launchctl refused to restart com.wisent.stado-precheck-runner'
if await_listening "$snapshot_path" "$snapshot_bytes" 90; then
  printf 'runner listener: listening for jobs\n'
  exit 0
fi

# A listener launchd no longer owns keeps the registration's session and
# writes nothing, so the managed job cannot take over and every job for these
# labels queues forever. That is the state this host was in on 2026-09-06: a
# `Runner.Listener` under the runner root, owned by stado-precheck, whose last
# log line was `Shutting down JobDispatcher` from a kickstart three quarters
# of an hour earlier.
#
# Only processes whose executable is UNDER THIS RUNNER'S ROOT are signalled,
# and only after the ordinary restart has already failed to produce a fresh
# listening line.
stale=$(/bin/ps -Ao pid=,comm= |
  /usr/bin/awk -v root="$runner_root/" '$2 ~ /Runner\.Listener$|runsvc\.sh$/ && index($2, root) == 1 { print $1 }')
if [ -z "$stale" ]; then
  last=$(root tail -n 5 "$(newest_log_path)" 2>/dev/null | /usr/bin/tr '\n' ' ')
  fail "the runner did not report listening within 90s of the restart and holds no stale listener to reclaim: $last"
fi
printf 'reclaiming stale listener pids: %s\n' "$(printf '%s' "$stale" | /usr/bin/tr '\n' ' ')"
for pid in $stale; do
  root kill -TERM "$pid" 2>/dev/null || true
done
sleep 10
for pid in $stale; do
  if /bin/ps -p "$pid" >/dev/null 2>&1; then
    root kill -KILL "$pid" 2>/dev/null || true
  fi
done
snapshot
root launchctl kickstart -k system/com.wisent.stado-precheck-runner ||
  fail 'launchctl refused to restart com.wisent.stado-precheck-runner after reclaiming its listener'
if await_listening "$snapshot_path" "$snapshot_bytes" 150; then
  printf 'runner listener: listening for jobs after reclaiming a stale listener\n'
  exit 0
fi
last=$(root tail -n 5 "$(newest_log_path)" 2>/dev/null | /usr/bin/tr '\n' ' ')
fail "the runner did not report listening after its stale listener was reclaimed: $last"
"#;

pub(crate) const MACOS_REMOVE: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_user=stado-precheck
runner_root=/Users/Shared/stado-precheck-runner
token=__TOKEN__
token_file=$(mktemp)
cleanup() { root rm -f "$token_file"; }
trap cleanup EXIT HUP INT TERM
root launchctl bootout system/com.wisent.stado-precheck-runner >/dev/null 2>&1 || true
if [ -f "$runner_root/.runner" ] && dscl . -read /Users/$runner_user >/dev/null 2>&1; then
  root chown -R "$runner_user:$runner_user" "$runner_root"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  root chown "$runner_user:$runner_user" "$token_file"
  root sudo -u "$runner_user" -H -- /usr/bin/env \
    HOME="$runner_root" PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin TOKEN_FILE="$token_file" \
    /bin/bash -c 'cd "$HOME"; read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; exec ./config.sh remove --unattended'
  token=
fi
root rm -f /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist
root pfctl -a com.wisent.stado-precheck -F all >/dev/null 2>&1 || true
root rm -f /etc/pf.anchors/com.wisent.stado-precheck
root rm -rf "$runner_root"
root dscl . -delete /Users/$runner_user >/dev/null 2>&1 || true
root dscl . -delete /Groups/$runner_user >/dev/null 2>&1 || true
printf 'runner service: removed\nrunner identity: removed\nnetwork boundary: removed\n'
"#;
