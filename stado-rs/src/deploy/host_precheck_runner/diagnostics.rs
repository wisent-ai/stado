//! What a runner that will not start is saying, read whole.
//!
//! `status` answers whether a runner is healthy and reduces its log to one
//! line, which is the right shape for a fleet table and the wrong shape for a
//! diagnosis: a .NET `System.IO.IOException: Permission denied` names the path
//! it could not open in the frames underneath that line, and those frames were
//! being dropped. On 2026-09-07 a repository-scoped runner on
//! `ubuntu-server-rtx-pro-6000` reported `activating` with an empty journal and
//! that one truncated line for an hour, because a GitHub runner's diagnosis
//! lives in `_diag/*.log` inside its own root and no product reader returned it.
//!
//! Everything here is read-only and takes a declared profile, so the paths, the
//! service account and the unit name are resolved from the declaration by
//! `profile_template` rather than typed by a caller. That is also why this
//! needs no `stado host exec` allowlist row: the allowlist is exact-match with
//! no operator-supplied path by construction, so it could never name a
//! per-profile runner root, while the audited host channel already resolves
//! one.

use serde_json::{json, Value};

use super::declaration::{runner_profile, runner_target};
use super::platform::{profile_template, Platform};
use crate::deploy::{host_channel, production_runner, DeployError};

/// Read the unit's own verdict and the newest diagnostic log, whole.
///
/// `systemctl show` is asked for exactly the properties that distinguish a
/// crash loop from a process that never started. `StandardOutput` and
/// `StandardError` are on that list because a unit whose output is pointed at
/// a file its account cannot create fails as `activating` with nothing in the
/// journal, and until a reader can see where it was pointed that state cannot
/// be told apart from any other quiet refusal.
const LINUX_DIAGNOSTICS: &str = r#"set -eu
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_root=/opt/wisent/stado-precheck-runner
runner_user=stado-precheck
root systemctl show -p ActiveState -p SubState -p Result -p NRestarts -p ExecMainStatus -p StandardOutput -p StandardError wisent-stado-precheck-runner.service || true
printf 'RunnerAccount=%s\n' "$(id -un "$runner_user" 2>/dev/null || printf 'absent')"
printf 'RunnerRoot=%s\n' "$runner_root"
newest=$(root sh -c "ls -t \"$runner_root\"/_diag/*.log 2>/dev/null | head -n 1" || true)
printf 'DiagnosticLog=%s\n' "${newest:-none}"
printf '%s\n' '--- tail ---'
if [ -n "$newest" ]; then root tail -n 120 "$newest"; fi
"#;

const MACOS_DIAGNOSTICS: &str = r#"set -eu
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_root=/Users/Shared/stado-precheck-runner
runner_user=stado-precheck
state=$(root launchctl print system/com.wisent.stado-precheck-runner 2>/dev/null || true)
printf 'ActiveState=%s\n' "$(printf '%s' "$state" | sed -n 's/.*state = \([a-z]*\).*/\1/p' | head -n 1)"
printf 'ExecMainStatus=%s\n' "$(printf '%s' "$state" | sed -n 's/.*last exit code = \([0-9-]*\).*/\1/p' | head -n 1)"
printf 'StandardOutput=%s\n' "$runner_root/_diag/launchd.stdout.log"
printf 'StandardError=%s\n' "$runner_root/_diag/launchd.stderr.log"
printf 'RunnerAccount=%s\n' "$(id -un "$runner_user" 2>/dev/null || printf 'absent')"
printf 'RunnerRoot=%s\n' "$runner_root"
newest=$(root sh -c "ls -t \"$runner_root\"/_diag/*.log 2>/dev/null | head -n 1" || true)
printf 'DiagnosticLog=%s\n' "${newest:-none}"
printf '%s\n' '--- tail ---'
if [ -n "$newest" ]; then root tail -n 120 "$newest"; fi
"#;

/// One `Key=value` line from the program's header half.
fn field(head: &str, key: &str) -> String {
    let prefix = format!("{key}=");
    head.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap_or("-")
        .to_string()
}

pub async fn diagnostics_declared(
    target_name: &str,
    profile_name: &str,
) -> Result<Value, DeployError> {
    let profile = runner_profile(profile_name)?;
    let target = runner_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    let script = profile_template(
        match platform {
            Platform::LinuxAmd64 => LINUX_DIAGNOSTICS,
            Platform::DarwinArm64 => MACOS_DIAGNOSTICS,
        },
        profile,
    );
    let output = host_channel::run_script(&target, &script, &production_runner()).await?;
    // The tail is reported even when the read failed: a program that could not
    // reach the unit still printed whatever it did reach, and discarding that
    // is the defect this command exists to end.
    let (head, tail) = output
        .stdout
        .split_once("--- tail ---\n")
        .unwrap_or((output.stdout.as_str(), ""));
    Ok(json!({
        "target": target.name,
        "profile": profile.name,
        "unit": profile.unit_label,
        "platform": platform.name(),
        "account": field(head, "RunnerAccount"),
        "runner_root": field(head, "RunnerRoot"),
        "active_state": field(head, "ActiveState"),
        "sub_state": field(head, "SubState"),
        "result": field(head, "Result"),
        "restarts": field(head, "NRestarts"),
        "exec_main_status": field(head, "ExecMainStatus"),
        "standard_output": field(head, "StandardOutput"),
        "standard_error": field(head, "StandardError"),
        "log": field(head, "DiagnosticLog"),
        "tail": tail.trim_end(),
        "read": if output.ok() { "complete" } else { "partial" },
        "stderr": output.stderr.trim(),
    }))
}
