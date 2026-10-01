//! What macOS reads back and removes for one installed runner. A restart is
//! not a program of the host: the listener is a role of the host's Stado
//! unit, and `restart` cycles that role from the caller's side.

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
# The listener is the `--precheck-runner` role of the host's one Stado unit,
# so there is no daemon of its own to read; what this host owns is the
# launcher that role runs. A daemon left by an earlier install is a second
# owner of the listener and is reported as such.
[ -x "$runner_root/start-runner.sh" ] ||
  fail "the runner launcher $runner_root/start-runner.sh is missing; \`stado runner install\` writes it"
if root launchctl print system/com.wisent.stado-precheck-runner >/dev/null 2>&1; then
  fail 'launchd daemon com.wisent.stado-precheck-runner still runs beside the Stado unit; `stado runner install` retires it'
fi
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
# check measured its own invocation rather than the product. What can be
# observed about the listener is the process the Stado unit's role keeps
# and the account that owns it.
# No `root` here: the process table is world-readable, and the host's
# passwordless sudo is granted for named commands only — asking for `ps`
# through it is refused, which is a fact about sudoers rather than about the
# runner.
# The whole table is consumed rather than cut short: `awk ... { exit }` closes
# the pipe while `ps` is still writing, `ps` dies of SIGPIPE, and `pipefail`
# then reports "the process table could not be read" on a host where reading
# it worked perfectly.
# Scoped to THIS runner's root. Matching any `Runner.Listener` would pass on
# the wrong process: a host can run several, one of which may run as the same
# account, so the check would report a healthy listener while the pre-check
# runner's own listener is dead and every job for these labels queues.
listener_owner=$(/bin/ps -Ao user=,comm= |
  /usr/bin/awk -v root="$runner_root/" \
    '$2 ~ /Runner\.Listener$/ && index($2, root) == 1 && !seen++ { owner = $1 } END { print owner }') ||
  fail 'the process table could not be read'
listener_problem=
if [ -z "$listener_owner" ]; then
  runner_log=$(root sh -c "ls -t \"$runner_root\"/_diag/Runner_*.log 2>/dev/null | head -n 1")
  runner_tail=$(root tail -n 3 "$runner_log" 2>/dev/null | /usr/bin/tr '\n' ' ')
  owned=$(/bin/ps -Ao user= | /usr/bin/grep -c -x 'stado-precheck')
  listener_problem="no listener is running from $runner_root, so this host takes no jobs for its labels; \`stado service status stado --host\` says whether the Stado unit runs its --precheck-runner role. stado-precheck holds $owned processes. runner log ($runner_log): $runner_tail"
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
# it, so a documentation gate sits queued for half an hour against a host
# whose daemon is `state = running`.
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
# about which one: an operator reading a queued job needs the distinction.
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
printf 'runner launcher: removed\nrunner identity: removed\nnetwork boundary: removed\n'
"#;
