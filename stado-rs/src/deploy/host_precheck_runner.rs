//! Rust-owned lifecycle for the isolated GitHub pre-check runner pool.
//!
//! Stado resolves the host from the canonical registry, obtains a short-lived
//! GitHub registration token through Skarbiec, and sends one fixed installer
//! program over the audited host channel. No Python helper or operator shell is
//! part of the lifecycle.

use std::time::Duration;

use serde_json::{json, Value};

use super::{host_channel, production_runner, CommandOutput, DeployError};
use crate::targets::ComputeTarget;

pub const GITHUB_ORGANIZATION: &str = "wisent-ai";
pub const GITHUB_CREDENTIAL_ITEM: &str = "platform-admin-github";
pub const RUNNER_GROUP: &str = "stado-precheck";
pub const RUNNER_USER: &str = "stado-precheck";
pub const RUNNER_VERSION: &str = "2.336.0";
pub const LINUX_SHA256: &str = "04cf0be1aff4c3ec3554466c39124ca250e3effd8873bb7e8d68535aa9505d5d";
pub const MACOS_SHA256: &str = "8e8839c49b7060b6b2154f4931f815df330c27f167d53ef2239ee3dfce28b079";

// These are network classes, not fleet addresses. Keeping the policy here makes
// the Linux nftables and macOS PF renderers consume one source of truth.
pub const BLOCKED_IPV4_NETWORKS: &[&str] = &[
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.168.0.0/16",
];
pub const BLOCKED_IPV6_NETWORKS: &[&str] = &["::1/128", "fc00::/7", "fe80::/10"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    LinuxAmd64,
    DarwinArm64,
}

impl Platform {
    fn for_target(target: &ComputeTarget) -> Result<Self, DeployError> {
        match target.release_platform.as_str() {
            "linux-amd64" => Ok(Self::LinuxAmd64),
            "darwin-arm64" => Ok(Self::DarwinArm64),
            other => Err(DeployError(format!(
                "target {:?} has unsupported precheck runner platform {:?}",
                target.name, other
            ))),
        }
    }
}

fn shell_list(values: &[&str]) -> String {
    values.join(", ")
}

fn replace(template: &str, pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .fold(template.to_string(), |text, (marker, value)| {
            text.replace(marker, value)
        })
}

fn linux_installer(target: &ComputeTarget, registration_token: &str) -> String {
    let runner_name = format!("stado-precheck-{}", target.name);
    replace(
        LINUX_INSTALLER,
        &[
            ("__VERSION__", RUNNER_VERSION.to_string()),
            ("__SHA256__", LINUX_SHA256.to_string()),
            ("__TOKEN__", super::shlex_quote(registration_token)),
            ("__RUNNER_NAME__", super::shlex_quote(&runner_name)),
            ("__RUNNER_GROUP__", super::shlex_quote(RUNNER_GROUP)),
            (
                "__ORGANIZATION_URL__",
                format!("https://github.com/{GITHUB_ORGANIZATION}"),
            ),
            ("__BLOCKED_IPV4__", shell_list(BLOCKED_IPV4_NETWORKS)),
            ("__BLOCKED_IPV6__", shell_list(BLOCKED_IPV6_NETWORKS)),
        ],
    )
}

fn macos_installer(target: &ComputeTarget, registration_token: &str) -> String {
    let runner_name = format!("stado-precheck-{}", target.name);
    replace(
        MACOS_INSTALLER,
        &[
            ("__VERSION__", RUNNER_VERSION.to_string()),
            ("__SHA256__", MACOS_SHA256.to_string()),
            ("__TOKEN__", super::shlex_quote(registration_token)),
            ("__RUNNER_NAME__", super::shlex_quote(&runner_name)),
            ("__RUNNER_GROUP__", super::shlex_quote(RUNNER_GROUP)),
            (
                "__ORGANIZATION_URL__",
                format!("https://github.com/{GITHUB_ORGANIZATION}"),
            ),
            (
                "__BLOCKED_NETWORKS__",
                BLOCKED_IPV4_NETWORKS
                    .iter()
                    .chain(BLOCKED_IPV6_NETWORKS.iter())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ],
    )
}

async fn github_runner_token(kind: &str) -> Result<String, DeployError> {
    let credential = crate::credential_store::read_string(GITHUB_CREDENTIAL_ITEM, "value")
        .await
        .map_err(|error| DeployError(error.to_string()))?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            DeployError(format!(
                "credential {GITHUB_CREDENTIAL_ITEM}.value is required"
            ))
        })?;
    let endpoint =
        format!("https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/runners/{kind}-token");
    let response = reqwest::Client::new()
        .post(endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::USER_AGENT, "wisent-stado-precheck-runner")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .bearer_auth(&credential)
        .send()
        .await
        .map_err(|error| DeployError(format!("GitHub runner token request failed: {error}")))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| DeployError(format!("GitHub runner token response failed: {error}")))?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
        return Err(DeployError(format!(
            "GitHub runner token request returned HTTP {}: {}",
            status.as_u16(),
            detail.trim()
        )));
    }
    serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| DeployError(format!("GitHub runner token response is invalid: {error}")))?
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| DeployError("GitHub runner token response has no token".to_string()))
}

fn report(target: &ComputeTarget, output: &CommandOutput, action: &str) -> Value {
    json!({
        "target": target.name,
        "platform": target.release_platform,
        "runner_group": RUNNER_GROUP,
        "action": action,
        "status": if output.ok() { "completed" } else { "failed" },
        "exit_code": output.code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    })
}

pub async fn install(target_name: &str) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    let token = github_runner_token("registration").await?;
    let script = match platform {
        Platform::LinuxAmd64 => linux_installer(&target, &token),
        Platform::DarwinArm64 => macos_installer(&target, &token),
    };
    let output = host_channel::run_script_with_timeout(
        &target,
        &script,
        Duration::from_secs(15 * 60),
        &production_runner(),
    )
    .await?;
    let value = report(&target, &output, "install");
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: precheck runner installation failed: {}",
            target.name,
            host_channel::last_error_line(&output, "remote installer failed")
        )));
    }
    Ok(value)
}

pub async fn status(target_name: &str) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let script = match Platform::for_target(&target)? {
        Platform::LinuxAmd64 => LINUX_STATUS,
        Platform::DarwinArm64 => MACOS_STATUS,
    };
    let output = host_channel::run_script(&target, script, &production_runner()).await?;
    let value = report(&target, &output, "status");
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: precheck runner status failed: {}",
            target.name,
            host_channel::last_error_line(&output, "remote status failed")
        )));
    }
    Ok(value)
}

pub async fn remove(target_name: &str) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    let token = github_runner_token("remove").await?;
    let script = replace(
        match platform {
            Platform::LinuxAmd64 => LINUX_REMOVE,
            Platform::DarwinArm64 => MACOS_REMOVE,
        },
        &[("__TOKEN__", super::shlex_quote(&token))],
    );
    let output = host_channel::run_script_with_timeout(
        &target,
        &script,
        Duration::from_secs(5 * 60),
        &production_runner(),
    )
    .await?;
    let value = report(&target, &output, "remove");
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: precheck runner removal failed: {}",
            target.name,
            host_channel::last_error_line(&output, "remote removal failed")
        )));
    }
    Ok(value)
}

const LINUX_INSTALLER: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
version=__VERSION__
expected=__SHA256__
token=__TOKEN__
runner_name=__RUNNER_NAME__
runner_group=__RUNNER_GROUP__
runner_user=stado-precheck
runner_root=/opt/wisent/stado-precheck-runner
archive=$(mktemp)
token_file=$(mktemp)
cleanup() { root rm -f "$archive" "$token_file"; }
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
  root mkdir -p "$runner_root/_work" "$runner_root/_diag"
  root chown "$runner_user:$runner_user" "$runner_root/_work" "$runner_root/_diag"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  root chown "$runner_user:$runner_user" "$token_file"
  root /usr/sbin/runuser --user "$runner_user" -- /usr/bin/env \
    HOME="$runner_root" PATH=/usr/local/bin:/usr/bin:/bin TOKEN_FILE="$token_file" \
    /bin/bash -c 'read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; export ACTIONS_RUNNER_INPUT_URL=__ORGANIZATION_URL__ ACTIONS_RUNNER_INPUT_NAME="$1" ACTIONS_RUNNER_INPUT_RUNNERGROUP="$2" ACTIONS_RUNNER_INPUT_LABELS=stado-precheck ACTIONS_RUNNER_INPUT_WORK=_work; exec ./config.sh --unattended --replace --disableupdate' \
    bash "$runner_name" "$runner_group"
  token=
fi

root chown -R root:root "$runner_root"
root chmod -R go-w "$runner_root"
root chown "$runner_user:$runner_user" "$runner_root/_work" "$runner_root/_diag"
root chmod 700 "$runner_root/_work" "$runner_root/_diag"

hook=$(mktemp)
cat > "$hook" <<'HOOK'
#!/bin/sh
set -eu
find /opt/wisent/stado-precheck-runner/_work -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
HOOK
root install -o root -g root -m 0755 "$hook" "$runner_root/clean-work.sh"
rm -f "$hook"

rules=$(mktemp)
cat > "$rules" <<RULES
table inet stado_precheck {
  chain output {
    type filter hook output priority filter; policy accept;
    meta skuid $uid ip daddr 127.0.0.53 udp dport 53 accept
    meta skuid $uid ip daddr 127.0.0.53 tcp dport 53 accept
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

unit=$(mktemp)
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
Environment=ACTIONS_RUNNER_HOOK_JOB_STARTED=$runner_root/clean-work.sh
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
ReadWritePaths=$runner_root/_work $runner_root/_diag

[Install]
WantedBy=multi-user.target
UNIT
root install -o root -g root -m 0644 "$unit" /etc/systemd/system/wisent-stado-precheck-runner.service
rm -f "$unit"
root systemctl daemon-reload
root systemctl enable --now wisent-stado-precheck-runner.service >/dev/null
root systemctl restart wisent-stado-precheck-runner.service
root systemctl is-active --quiet wisent-stado-precheck-runner.service
printf 'runner service: active\nrunner identity: %s uid=%s\nrunner group: %s\nprivate-network egress: blocked\n' "$runner_user" "$uid" "$runner_group"
"#;

const MACOS_INSTALLER: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
version=__VERSION__
expected=__SHA256__
token=__TOKEN__
runner_name=__RUNNER_NAME__
runner_group=__RUNNER_GROUP__
runner_user=stado-precheck
runner_root=/Users/Shared/stado-precheck-runner
archive=$(mktemp)
token_file=$(mktemp)
cleanup() { root rm -f "$archive" "$token_file"; }
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

if [ ! -f "$runner_root/.runner" ]; then
  curl --fail --silent --show-error --location --max-time 120 \
    "https://github.com/actions/runner/releases/download/v$version/actions-runner-osx-arm64-$version.tar.gz" \
    -o "$archive"
  actual=$(shasum -a 256 "$archive" | cut -d' ' -f1)
  [ "$actual" = "$expected" ] || { printf '%s\n' "runner checksum mismatch: $actual" >&2; exit 1; }
  root rm -rf "$runner_root"
  root mkdir -p "$runner_root"
  root tar -xzf "$archive" -C "$runner_root"
  root codesign --remove-signature "$runner_root/bin/Runner.Listener"
  root chown -R "$runner_user:$runner_user" "$runner_root"
  root mkdir -p "$runner_root/_work" "$runner_root/_diag"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  root chown "$runner_user:$runner_user" "$token_file"
  root sudo -u "$runner_user" -H -- /usr/bin/env \
    HOME="$runner_root" PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin TOKEN_FILE="$token_file" \
    /bin/bash -c 'cd "$HOME"; read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; export ACTIONS_RUNNER_INPUT_URL=__ORGANIZATION_URL__ ACTIONS_RUNNER_INPUT_NAME="$1" ACTIONS_RUNNER_INPUT_RUNNERGROUP="$2" ACTIONS_RUNNER_INPUT_LABELS=stado-precheck ACTIONS_RUNNER_INPUT_WORK=_work; exec ./config.sh --unattended --replace --disableupdate' \
    bash "$runner_name" "$runner_group"
  token=
fi

root chown -R root:wheel "$runner_root"
root chmod -R go-w "$runner_root"
root chown "$runner_user:$runner_user" "$runner_root/_work" "$runner_root/_diag"
root chmod 700 "$runner_root/_work" "$runner_root/_diag"

hook=$(mktemp)
cat > "$hook" <<'HOOK'
#!/bin/sh
set -eu
find /Users/Shared/stado-precheck-runner/_work -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
HOOK
root install -o root -g wheel -m 0755 "$hook" "$runner_root/clean-work.sh"
rm -f "$hook"

anchor=$(mktemp)
cat > "$anchor" <<RULES
block return out quick proto { tcp udp } from any to { __BLOCKED_NETWORKS__ } user $runner_user
RULES
root install -o root -g wheel -m 0644 "$anchor" /etc/pf.anchors/com.wisent.stado-precheck
rm -f "$anchor"

launcher=$(mktemp)
cat > "$launcher" <<LAUNCHER
#!/bin/sh
set -eu
/sbin/pfctl -a com.wisent.stado-precheck -f /etc/pf.anchors/com.wisent.stado-precheck
/sbin/pfctl -E >/dev/null 2>&1 || true
exec /usr/bin/sudo -u $runner_user -H -- /usr/bin/env HOME=$runner_root PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin ACTIONS_RUNNER_HOOK_JOB_STARTED=$runner_root/clean-work.sh ACTIONS_RUNNER_HOOK_JOB_COMPLETED=$runner_root/clean-work.sh $runner_root/bin/runsvc.sh
LAUNCHER
root install -o root -g wheel -m 0755 "$launcher" "$runner_root/start-runner.sh"
rm -f "$launcher"

plist=$(mktemp)
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
root install -o root -g wheel -m 0644 "$plist" /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist
rm -f "$plist"
root launchctl bootout system/com.wisent.stado-precheck-runner >/dev/null 2>&1 || true
root launchctl bootstrap system /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist
root launchctl enable system/com.wisent.stado-precheck-runner
root launchctl kickstart -k system/com.wisent.stado-precheck-runner
root launchctl print system/com.wisent.stado-precheck-runner | grep -F 'state = running' >/dev/null
printf 'runner service: running\nrunner identity: %s uid=%s\nrunner group: %s\nprivate-network egress: blocked\n' "$runner_user" "$uid" "$runner_group"
"#;

const LINUX_STATUS: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
root systemctl is-active wisent-stado-precheck-runner.service
root systemctl is-enabled wisent-stado-precheck-runner.service
id stado-precheck
root nft list table inet stado_precheck
"#;

const MACOS_STATUS: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
root launchctl print system/com.wisent.stado-precheck-runner
dscl . -read /Users/stado-precheck UniqueID PrimaryGroupID NFSHomeDirectory UserShell Password
root pfctl -a com.wisent.stado-precheck -sr
"#;

const LINUX_REMOVE: &str = r#"set -euo pipefail
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
printf 'runner service: removed\nrunner identity: removed\nnetwork boundary: removed\n'
"#;

const MACOS_REMOVE: &str = r#"set -euo pipefail
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
