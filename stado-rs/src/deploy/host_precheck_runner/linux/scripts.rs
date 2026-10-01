//! What Linux reads back and removes for one installed runner. A restart is
//! not a program of the host: the listener is a role of the host's Stado
//! unit, and `restart` cycles that role from the caller's side.

pub(crate) const LINUX_PUBLISHER_STATUS: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_root=/opt/wisent/stado-precheck-runner
runner_user=stado-precheck
# The listener is the `--precheck-runner` role of the host's one Stado unit;
# what this host owns is the launcher that role runs. A service left by an
# earlier install is a second owner of the listener.
[ -x "$runner_root/start-runner.sh" ]
! root systemctl is-active --quiet wisent-stado-precheck-runner.service
id "$runner_user" >/dev/null
root nft list table inet stado_precheck >/dev/null
scope=$(root sed -n '3p' "$runner_root/.stado/registered-runner" 2>/dev/null || true)
newest_log=$(root sh -c "ls -t \"$runner_root\"/_diag/Runner_*.log 2>/dev/null | head -n 1")
if [ -n "$newest_log" ]; then
  listener_state=$(root tail -n 400 "$newest_log" | grep -a -E 'Listening for Jobs|Running job|Job .* completed|Terminate|Error|Exception' | tail -n 1 || true)
  [ -n "$listener_state" ] || listener_state='no listener event in the last 400 log lines'
else
  listener_state='no runner diagnostic log, so the listener has never started'
fi
job_holder=none
for marker in /opt/wisent/.stado-runner-jobs/*.job; do
  [ -f "$marker" ] || continue
  pid=$(root head -n 1 "$marker" 2>/dev/null || true)
  case "$pid" in ''|*[!0-9]*) continue ;; esac
  if kill -0 "$pid" 2>/dev/null; then job_holder="$(basename "$marker" .job) pid=$pid"; else job_holder="$(basename "$marker" .job) stale"; fi
done
printf 'listener: %s\nhost job slot: %s\n%s\n' "$listener_state" "$job_holder" "${scope:-unrecorded}"
"#;

pub(crate) const LINUX_STATUS: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
[ -x /opt/wisent/stado-precheck-runner/start-runner.sh ]
! root systemctl is-active --quiet wisent-stado-precheck-runner.service
id stado-precheck
root nft list table inet stado_precheck
agent_id=$(root cat /opt/wisent/stado-precheck-runner/routes/kronika-agent-id)
[ -n "$agent_id" ]
brama_route=$(root cat /opt/wisent/stado-precheck-runner/routes/brama.url)
[ -n "$brama_route" ]
secret_meta=$(root stat -c '%U %G %a' /opt/wisent/stado-precheck-runner/.stado/kronika-agent-auth-secret)
[ "$secret_meta" = "stado-precheck stado-precheck 600" ]
# Which door this runner registered through. GitHub's own runner list needs an
# organization-admin token, so the scope is read from the record the installer
# wrote beside the runner.
scope=$(root sed -n '3p' /opt/wisent/stado-precheck-runner/.stado/registered-runner 2>/dev/null || true)
# Which runner holds the host's single job slot, if any. A marker whose worker
# is gone is a crash and not a slot, so the reader says so rather than naming
# a holder that no longer exists.
job_holder=none
for marker in /opt/wisent/.stado-runner-jobs/*.job; do
  [ -f "$marker" ] || continue
  pid=$(root head -n 1 "$marker" 2>/dev/null || true)
  case "$pid" in ''|*[!0-9]*) continue ;; esac
  if kill -0 "$pid" 2>/dev/null; then job_holder="$(basename "$marker" .job) pid=$pid"; else job_holder="$(basename "$marker" .job) stale"; fi
done
newest_log=$(root sh -c "ls -t /opt/wisent/stado-precheck-runner/_diag/Runner_*.log 2>/dev/null | head -n 1")
if [ -n "$newest_log" ]; then
  listener_state=$(root tail -n 400 "$newest_log" | grep -a -E 'Listening for Jobs|Running job|Job .* completed|Terminate|Error|Exception' | tail -n 1 || true)
  [ -n "$listener_state" ] || listener_state='no listener event in the last 400 log lines'
else
  listener_state='no runner diagnostic log, so the listener has never started'
fi
printf 'kronika agent: %s\nbrama route: %s\nkronika signing secret: owner=%s\nlistener: %s\nhost job slot: %s\n%s\n' "$agent_id" "$brama_route" "$secret_meta" "$listener_state" "$job_holder" "${scope:-unrecorded}"
"#;

pub(crate) const LINUX_REMOVE: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_user=stado-precheck
runner_root=/opt/wisent/stado-precheck-runner
token=__TOKEN__
token_file=$(mktemp)
cleanup() { root rm -f "$token_file"; }
trap cleanup EXIT HUP INT TERM
root systemctl disable --now wisent-stado-precheck-runner.service >/dev/null 2>&1 || true
if [ -f "$runner_root/.runner" ] && id "$runner_user" >/dev/null 2>&1; then
  root chown -R "$runner_user:$runner_user" "$runner_root"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  root chown "$runner_user:$runner_user" "$token_file"
  root /usr/sbin/runuser --user "$runner_user" -- /usr/bin/env \
    HOME="$runner_root" PATH=/usr/local/bin:/usr/bin:/bin TOKEN_FILE="$token_file" \
    /bin/bash -c 'cd "$HOME"; read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; exec ./config.sh remove --unattended'
  token=
fi
root rm -f /etc/systemd/system/wisent-stado-precheck-runner.service
root systemctl daemon-reload
root nft delete table inet stado_precheck >/dev/null 2>&1 || true
root rm -f /etc/nftables.d/stado_precheck.nft
if [ -f /etc/nftables.conf ]; then
  root sed -i '\|include "/etc/nftables.d/stado_precheck.nft"|d' /etc/nftables.conf
fi
root rm -rf "$runner_root"
root /usr/sbin/userdel "$runner_user" >/dev/null 2>&1 || true
root /usr/sbin/groupdel "$runner_user" >/dev/null 2>&1 || true
printf 'runner launcher: removed\nrunner identity: removed\nnetwork boundary: removed\n'
"#;
