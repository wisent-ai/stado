//! The Linux program that installs, registers and confines one runner.

pub(crate) const LINUX_INSTALLER: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
version=__VERSION__
expected=__SHA256__
token=__TOKEN__
runner_name=__RUNNER_NAME__
restart_registered=__RESTART_REGISTERED__
runner_group=__RUNNER_GROUP__
runner_user=stado-precheck
runner_root=/opt/wisent/stado-precheck-runner
mkdir -p "$HOME/.stado/work"
staging=$(mktemp -d "$HOME/.stado/work/runner-install.XXXXXX")
archive=$(mktemp "$staging/archive.XXXXXX")
token_file=$(mktemp "$staging/token.XXXXXX")
cleanup() { root rm -f "$archive" "$token_file"; root rm -rf "$staging"; }
trap cleanup EXIT HUP INT TERM

if ! getent group "$runner_user" >/dev/null; then root /usr/sbin/groupadd --system "$runner_user"; fi
if ! id "$runner_user" >/dev/null 2>&1; then
  root /usr/sbin/useradd --system --gid "$runner_user" --home-dir "$runner_root" --no-create-home --shell /usr/sbin/nologin "$runner_user"
fi
uid=$(id -u "$runner_user")
[ "$uid" -ne 0 ] || { printf '%s\n' 'runner account is root' >&2; exit 1; }
for privileged in sudo wheel admin; do
  if id -nG "$runner_user" | tr ' ' '\n' | grep -Fx "$privileged" >/dev/null; then
    printf '%s\n' "runner account belongs to $privileged" >&2
    exit 1
  fi
done

if [ ! -f "$runner_root/.runner" ]; then
  curl --fail --silent --show-error --location --max-time 120 \
    "https://github.com/actions/runner/releases/download/v$version/actions-runner-linux-x64-$version.tar.gz" \
    -o "$archive"
  actual=$(sha256sum "$archive" | cut -d' ' -f1)
  [ "$actual" = "$expected" ] || { printf '%s\n' "runner checksum mismatch: $actual" >&2; exit 1; }
  root rm -rf "$runner_root"
  root mkdir -p "$runner_root"
  root tar -xzf "$archive" -C "$runner_root" --no-same-owner
  root chown -R "$runner_user:$runner_user" "$runner_root"
  root mkdir -p "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"
  root chown "$runner_user:$runner_user" "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  root install -o "$runner_user" -g "$runner_user" -m 0600 "$token_file" "$runner_root/.registration-token"
  token_file="$runner_root/.registration-token"
  if ! (cd "$runner_root" && root /usr/sbin/runuser --user "$runner_user" -- /usr/bin/env \
    HOME="$runner_root" PATH=/usr/local/bin:/usr/bin:/bin TOKEN_FILE="$token_file" \
    /bin/bash -c 'read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; export ACTIONS_RUNNER_INPUT_URL=__REGISTRATION_URL__ ACTIONS_RUNNER_INPUT_NAME="$1" ACTIONS_RUNNER_INPUT_LABELS=__RUNNER_LABELS__ ACTIONS_RUNNER_INPUT_WORK=_work; [ -n "$2" ] && export ACTIONS_RUNNER_INPUT_RUNNERGROUP="$2"; exec ./config.sh --unattended --replace --disableupdate' \
    bash "$runner_name" "$runner_group"); then
    for log in "$runner_root"/_diag/Runner_*.log; do
      [ -f "$log" ] || continue
      root tail -n 80 "$log" >&2 || true
    done
    exit 1
  fi
  token=
  # The same record the darwin installer keeps: what this registration
  # answers for and which door it went through, so a later install sees a
  # moved declaration and `status` can say the scope without asking GitHub.
  root mkdir -p "$runner_root/.stado"
  printf '%s\n%s\n%s\n' __RUNNER_LABELS__ "$runner_group" __RUNNER_SCOPE__ | root tee "$runner_root/.stado/registered-runner" >/dev/null
fi

root mkdir -p "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"
root chown -R root:root "$runner_root"
root chmod -R go-w "$runner_root"
root chown -R "$runner_user:$runner_user" "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"
root chmod 700 "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"

root mkdir -p "$runner_root/routes"
printf '%s\n' __BRAMA_URL__ | root tee "$runner_root/routes/brama.url" >/dev/null
printf '%s\n' __KRONIKA_AGENT_ID__ | root tee "$runner_root/routes/kronika-agent-id" >/dev/null
root chown -R root:root "$runner_root/routes"
root chmod 555 "$runner_root/routes"
root chmod 444 "$runner_root/routes/brama.url"
root chmod 444 "$runner_root/routes/kronika-agent-id"

hook=$(mktemp "$staging/hook.XXXXXX")
cat > "$hook" <<'HOOK'
#!/bin/sh
set -eu
find /opt/wisent/stado-precheck-runner/_work -mindepth 1 -maxdepth 1 ! -name '_*' -exec rm -rf -- {} +
rm -f /opt/wisent/.stado-runner-jobs/$(id -un).job 2>/dev/null || true
HOOK
root install -o root -g root -m 0755 "$hook" "$runner_root/clean-work.sh"
rm -f "$hook"

# One job at a time on this host, across every runner registered on it. The
# program is `job_gate_program`, so the shell that runs on a host and the
# shell a test drives are the same text.
gate=$(mktemp "$staging/gate.XXXXXX")
cat > "$gate" <<'GATE'
__JOB_GATE__
GATE
root install -o root -g root -m 0755 "$gate" "$runner_root/job-gate.sh"
root mkdir -p /opt/wisent/.stado-runner-jobs
root chmod 1777 /opt/wisent/.stado-runner-jobs
rm -f "$gate"

rules=$(mktemp "$staging/rules.XXXXXX")
cat > "$rules" <<RULES
table inet stado_precheck {
  chain output {
    type filter hook output priority filter; policy accept;
    meta skuid $uid ip daddr 127.0.0.53 udp dport 53 accept
    meta skuid $uid ip daddr 127.0.0.53 tcp dport 53 accept
    meta skuid $uid ip daddr 127.0.0.1 tcp dport __BRAMA_PORT__ accept
    meta skuid $uid ip daddr { __BLOCKED_IPV4__ } reject
    meta skuid $uid ip6 daddr { __BLOCKED_IPV6__ } reject
  }
}
RULES
root mkdir -p /etc/nftables.d
root install -o root -g root -m 0644 "$rules" /etc/nftables.d/stado_precheck.nft
root nft delete table inet stado_precheck >/dev/null 2>&1 || true
root nft -f /etc/nftables.d/stado_precheck.nft
if [ ! -f /etc/nftables.conf ]; then printf '%s\n' '#!/usr/sbin/nft -f' | root tee /etc/nftables.conf >/dev/null; fi
if ! root grep -F 'include "/etc/nftables.d/stado_precheck.nft"' /etc/nftables.conf >/dev/null; then
  printf '%s\n' 'include "/etc/nftables.d/stado_precheck.nft"' | root tee -a /etc/nftables.conf >/dev/null
fi
root systemctl enable nftables.service >/dev/null
rm -f "$rules"

unit=$(mktemp "$staging/unit.XXXXXX")
cat > "$unit" <<UNIT
[Unit]
Description=Wisent isolated GitHub pre-check runner
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$runner_user
Group=$runner_user
WorkingDirectory=$runner_root
ExecStartPre=$runner_root/clean-work.sh
ExecStart=$runner_root/bin/runsvc.sh
Restart=always
RestartSec=5
Environment=ACTIONS_RUNNER_HOOK_JOB_STARTED=$runner_root/job-gate.sh
Environment=ACTIONS_RUNNER_HOOK_JOB_COMPLETED=$runner_root/clean-work.sh
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=read-only
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=true
RestrictSUIDSGID=true
LockPersonality=true
ReadWritePaths=$runner_root/_work $runner_root/_diag $runner_root/.npm $runner_root/.cache $runner_root/.cargo $runner_root/.rustup /opt/wisent/.stado-runner-jobs

[Install]
WantedBy=multi-user.target
UNIT
service_changed=0
if [ ! -f "$runner_root/.service-reconciled" ]; then service_changed=1; fi
if [ ! -f /etc/systemd/system/wisent-stado-precheck-runner.service ] || ! root cmp -s "$unit" /etc/systemd/system/wisent-stado-precheck-runner.service; then
  service_changed=1
fi
root install -o root -g root -m 0644 "$unit" /etc/systemd/system/wisent-stado-precheck-runner.service
rm -f "$unit"
root systemctl daemon-reload
if root systemctl is-active --quiet wisent-stado-precheck-runner.service; then
  if [ "$service_changed" -eq 1 ] || [ "$restart_registered" -eq 1 ]; then
    root systemctl restart wisent-stado-precheck-runner.service
  fi
else
  root systemctl enable --now wisent-stado-precheck-runner.service >/dev/null
fi
root systemctl is-active --quiet wisent-stado-precheck-runner.service
root touch "$runner_root/.service-reconciled"
# What this host now carries, read back from its own record rather than from
# the declaration that asked for it: an install that skipped registration used
# to report the scope it would have used.
root sed -n '3p' "$runner_root/.stado/registered-runner" 2>/dev/null || true
job_holder=none
for marker in /opt/wisent/.stado-runner-jobs/*.job; do
  [ -f "$marker" ] || continue
  pid=$(root head -n 1 "$marker" 2>/dev/null || true)
  case "$pid" in ''|*[!0-9]*) continue ;; esac
  if kill -0 "$pid" 2>/dev/null; then job_holder="$(basename "$marker" .job) pid=$pid"; else job_holder="$(basename "$marker" .job) stale"; fi
done
printf 'host job slot: %s\n' "$job_holder"
printf 'runner service: active\nrunner identity: %s uid=%s\nrunner group: %s\nprivate-network egress: blocked except Stado route %s\n' "$runner_user" "$uid" "$runner_group" __BRAMA_URL__
"#;
