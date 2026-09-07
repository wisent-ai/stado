//! The macOS program that installs, registers and confines one runner.

pub(crate) const MACOS_INSTALLER: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
__MACOS_RUNTIME_FUNCTIONS__
version=__VERSION__
expected=__SHA256__
token=__TOKEN__
runner_name=__RUNNER_NAME__
runner_group=__RUNNER_GROUP__
restart_registered=__RESTART_REGISTERED__
runner_user=stado-precheck
runner_root=/Users/Shared/stado-precheck-runner
mkdir -p "$HOME/.stado/work"
staging=$(mktemp -d "$HOME/.stado/work/runner-install.XXXXXX")
archive=$(mktemp "$staging/archive.XXXXXX")
token_file=$(mktemp "$staging/token.XXXXXX")
cleanup() { root rm -f "$archive" "$token_file"; root rm -rf "$staging"; }
trap cleanup EXIT HUP INT TERM

if ! dscl . -read /Groups/$runner_user >/dev/null 2>&1; then
  used=$(dscl . -list /Users UniqueID; dscl . -list /Groups PrimaryGroupID)
  gid=450
  while printf '%s\n' "$used" | grep -E "[[:space:]]$gid$" >/dev/null; do gid=$((gid + 1)); done
  root dscl . -create /Groups/$runner_user
  root dscl . -create /Groups/$runner_user PrimaryGroupID "$gid"
  root dscl . -create /Groups/$runner_user RealName 'Wisent precheck runner'
fi
gid=$(dscl . -read /Groups/$runner_user PrimaryGroupID | awk '{print $2}')
if ! dscl . -read /Users/$runner_user >/dev/null 2>&1; then
  used=$(dscl . -list /Users UniqueID; dscl . -list /Groups PrimaryGroupID)
  uid=450
  while printf '%s\n' "$used" | grep -E "[[:space:]]$uid$" >/dev/null; do uid=$((uid + 1)); done
  root dscl . -create /Users/$runner_user
  root dscl . -create /Users/$runner_user UniqueID "$uid"
  root dscl . -create /Users/$runner_user PrimaryGroupID "$gid"
  root dscl . -create /Users/$runner_user NFSHomeDirectory "$runner_root"
  root dscl . -create /Users/$runner_user UserShell /bin/sh
  root dscl . -create /Users/$runner_user IsHidden 1
fi
root dscl . -create /Users/$runner_user Password '*'
uid=$(dscl . -read /Users/$runner_user UniqueID | awk '{print $2}')
[ "$uid" -ne 0 ] || { printf '%s\n' 'runner account is root' >&2; exit 1; }
if dseditgroup -o checkmember -m "$runner_user" admin | grep -qi 'yes'; then
  printf '%s\n' 'runner account belongs to admin' >&2
  exit 1
fi

runner_registered=0
if [ -f "$runner_root/.runner" ]; then runner_registered=1; fi
# Preserve GitHub's signatures. A registered runner may have advanced beyond
# the installer's pinned version, so restore apphosts from its own release.
runtime_repaired=0
if [ "$runner_registered" -eq 1 ] && ! runner_signatures_valid; then
  runtime_repaired=1
  resolve_runner_release
fi
if [ "$runner_registered" -eq 0 ] || [ "$runtime_repaired" -eq 1 ]; then
  fetch_runner_archive
fi
if [ "$runner_registered" -eq 0 ]; then
  root rm -rf "$runner_root"
  root mkdir -p "$runner_root"
  root tar -xzf "$archive" -C "$runner_root"
  installer_user=$(id -un)
  installer_group=$(id -gn)
  root chown -R "$installer_user:$installer_group" "$runner_root"
  root mkdir -p "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library/Caches" "$runner_root/.stado"
  root chown "$installer_user:$installer_group" "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library" "$runner_root/.stado"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  if ! (cd "$runner_root" && /usr/bin/env \
    HOME="$runner_root" TMPDIR="$runner_root/.tmp" DOTNET_BUNDLE_EXTRACT_BASE_DIR="$runner_root/.dotnet" PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin TOKEN_FILE="$token_file" \
    /bin/bash -c 'cd "$HOME"; read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; export ACTIONS_RUNNER_INPUT_URL=__REGISTRATION_URL__ ACTIONS_RUNNER_INPUT_NAME="$1" ACTIONS_RUNNER_INPUT_LABELS=__RUNNER_LABELS__ ACTIONS_RUNNER_INPUT_WORK=_work; [ -n "$2" ] && export ACTIONS_RUNNER_INPUT_RUNNERGROUP="$2"; exec ./config.sh --unattended --replace --disableupdate' \
    bash "$runner_name" "$runner_group"); then
    for log in "$runner_root"/_diag/Runner_*.log; do
      [ -f "$log" ] || continue
      root tail -n 80 "$log" >&2 || true
    done
    exit 1
  fi
  token=
  # What this registration answers for and which door it went through, so a
  # later install can see that the declaration moved and `status` can name the
  # scope. Labels are fixed at `config.sh` time and GitHub's own runner list
  # needs an organization-admin token to read, so the record lives beside the
  # runner.
  printf '%s\n%s\n%s\n' __RUNNER_LABELS__ "$runner_group" __RUNNER_SCOPE__ | root tee "$runner_root/.stado/registered-runner" >/dev/null
elif [ "$runtime_repaired" -eq 1 ]; then
  restore_runner_apphosts
fi

root mkdir -p "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library/Caches" "$runner_root/.stado"
root chown -R root:wheel "$runner_root"
root chmod -R go-w "$runner_root"
for state_file in "$runner_root"/.credentials* "$runner_root"/.runner "$runner_root"/.service "$runner_root"/.path; do
  [ -f "$state_file" ] || continue
  root chown "$runner_user:$runner_user" "$state_file"
  root chmod 600 "$state_file"
done
root chown -R "$runner_user:$runner_user" "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library" "$runner_root/.stado"
root chmod 700 "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library" "$runner_root/Library/Caches" "$runner_root/.stado"

root mkdir -p "$runner_root/routes"
printf '%s\n' __BRAMA_URL__ | root tee "$runner_root/routes/brama.url" >/dev/null
printf '%s\n' __KRONIKA_AGENT_ID__ | root tee "$runner_root/routes/kronika-agent-id" >/dev/null
root chown -R root:wheel "$runner_root/routes"
root chmod 555 "$runner_root/routes"
root chmod 444 "$runner_root/routes/brama.url"
root chmod 444 "$runner_root/routes/kronika-agent-id"

hook=$(mktemp "$staging/hook.XXXXXX")
cat > "$hook" <<'HOOK'
#!/bin/sh
set -eu
find /Users/Shared/stado-precheck-runner/_work -mindepth 1 -maxdepth 1 ! -name '_*' -exec rm -rf -- {} +
rm -f /Users/Shared/.stado-runner-jobs/$(id -un).job 2>/dev/null || true
HOOK
root install -o root -g wheel -m 0755 "$hook" "$runner_root/clean-work.sh"
rm -f "$hook"

# One job at a time on this host, across every runner registered on it. The
# program is `job_gate_program`, so the shell a host runs and the shell a test
# drives are the same text.
gate=$(mktemp "$staging/gate.XXXXXX")
cat > "$gate" <<'GATE'
__JOB_GATE__
GATE
root install -o root -g wheel -m 0755 "$gate" "$runner_root/job-gate.sh"
rm -f "$gate"

anchor=$(mktemp "$staging/anchor.XXXXXX")
cat > "$anchor" <<RULES
pass out quick proto tcp from any to 127.0.0.1 port __BRAMA_PORT__ user $runner_user
block return out quick proto { tcp udp } from any to { __BLOCKED_NETWORKS__ } user $runner_user
RULES
root install -o root -g wheel -m 0644 "$anchor" /etc/pf.anchors/com.wisent.stado-precheck
root pfctl -a com.wisent.stado-precheck -f /etc/pf.anchors/com.wisent.stado-precheck
root pfctl -E >/dev/null 2>&1 || true
rm -f "$anchor"

service_changed=$runtime_repaired
if [ ! -f "$runner_root/.service-reconciled" ]; then service_changed=1; fi

launcher=$(mktemp "$staging/launcher.XXXXXX")
cat > "$launcher" <<LAUNCHER
#!/bin/sh
set -eu
/sbin/pfctl -a com.wisent.stado-precheck -f /etc/pf.anchors/com.wisent.stado-precheck
/sbin/pfctl -E >/dev/null 2>&1 || true
exec /usr/bin/sudo -u $runner_user -H -- /usr/bin/env HOME=$runner_root TMPDIR=$runner_root/.tmp DOTNET_BUNDLE_EXTRACT_BASE_DIR=$runner_root/.dotnet PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin ACTIONS_RUNNER_HOOK_JOB_STARTED=$runner_root/job-gate.sh ACTIONS_RUNNER_HOOK_JOB_COMPLETED=$runner_root/clean-work.sh $runner_root/bin/runsvc.sh
LAUNCHER
if [ ! -f "$runner_root/start-runner.sh" ] || ! root cmp -s "$launcher" "$runner_root/start-runner.sh"; then
  service_changed=1
fi
root install -o root -g wheel -m 0755 "$launcher" "$runner_root/start-runner.sh"
rm -f "$launcher"

plist=$(mktemp "$staging/plist.XXXXXX")
cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.wisent.stado-precheck-runner</string>
<key>ProgramArguments</key><array><string>$runner_root/start-runner.sh</string></array>
<key>WorkingDirectory</key><string>$runner_root</string>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>5</integer>
<key>ProcessType</key><string>Background</string>
<key>StandardOutPath</key><string>$runner_root/_diag/launchd.stdout.log</string>
<key>StandardErrorPath</key><string>$runner_root/_diag/launchd.stderr.log</string>
</dict></plist>
PLIST
root plutil -lint "$plist" >/dev/null
if [ ! -f /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist ] || ! root cmp -s "$plist" /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist; then
  service_changed=1
fi
root install -o root -g wheel -m 0644 "$plist" /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist
rm -f "$plist"
if root launchctl print system/com.wisent.stado-precheck-runner >/dev/null 2>&1; then
  if [ "$service_changed" -eq 1 ] ||
     [ "$restart_registered" -eq 1 ] ||
     ! root launchctl print system/com.wisent.stado-precheck-runner |
       grep -F 'state = running' >/dev/null; then
    # GitHub, rather than launchd's outer RunnerService process, is the
    # authoritative listener health signal. Only a registered publisher that
    # GitHub reported offline reaches this branch.
    root launchctl kickstart -k system/com.wisent.stado-precheck-runner
  fi
else
  root launchctl bootstrap system /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist
fi
root launchctl enable system/com.wisent.stado-precheck-runner
root launchctl print system/com.wisent.stado-precheck-runner | grep -F 'state = running' >/dev/null
root touch "$runner_root/.service-reconciled"
# The same read-back the linux installer prints, for the same reason.
root sed -n '3p' "$runner_root/.stado/registered-runner" 2>/dev/null || true
job_holder=none
for marker in /Users/Shared/.stado-runner-jobs/*.job; do
  [ -f "$marker" ] || continue
  pid=$(root head -n 1 "$marker" 2>/dev/null || true)
  case "$pid" in ''|*[!0-9]*) continue ;; esac
  if kill -0 "$pid" 2>/dev/null; then job_holder="$(basename "$marker" .job) pid=$pid"; else job_holder="$(basename "$marker" .job) stale"; fi
done
printf 'host job slot: %s\n' "$job_holder"
printf 'runner service: running\nrunner identity: %s uid=%s\nrunner group: %s\nprivate-network egress: blocked except Stado route %s\n' "$runner_user" "$uid" "$runner_group" __BRAMA_URL__
"#;
