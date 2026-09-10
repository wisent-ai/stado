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
///
/// The memory lines exist for the failure this fleet actually keeps hitting:
/// `Failed to create CoreCLR, HRESULT: 0x8007000C` with exit 137 is the .NET
/// runtime refusing to reserve its heap, and a reader who sees only that line
/// cannot tell a machine out of memory from a unit whose own limits are too
/// small. Both readings are taken here, on the host, in the same pass as the
/// log.
const LINUX_DIAGNOSTICS: &str = r#"set -eu
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_root=/opt/wisent/stado-precheck-runner
runner_user=stado-precheck
root systemctl show -p ActiveState -p SubState -p Result -p NRestarts -p ExecMainStatus -p StandardOutput -p StandardError wisent-stado-precheck-runner.service || true
printf 'UnitLimits=%s\n' "$(root systemctl show -p MemoryMax -p MemoryHigh -p MemorySwapMax -p LimitAS -p TasksMax wisent-stado-precheck-runner.service 2>/dev/null | tr '\n' ' ')"
printf 'MemoryAvailableKB=%s\n' "$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)"
printf 'MemoryTotalKB=%s\n' "$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)"
printf 'SwapUsage=%s\n' "$(awk '/^Swap(Total|Free):/ {printf "%s=%s ", $1, $2}' /proc/meminfo)"
printf 'RunnerAccount=%s\n' "$(id -un "$runner_user" 2>/dev/null || printf 'absent')"
printf 'RunnerRoot=%s\n' "$runner_root"
newest=$(root sh -c "ls -t \"$runner_root\"/_diag/*.log 2>/dev/null | head -n 1" || true)
printf 'HostTime=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf 'LogModifiedAt=%s\n' "$(root sh -c "[ -n \"$newest\" ] && date -u -r \"$newest\" +%Y-%m-%dT%H:%M:%SZ" 2>/dev/null || printf 'unknown')"
printf 'WrapperLogModifiedAt=%s\n' "unknown"
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
printf 'UnitLimits=%s\n' "$(printf '%s' "$state" | grep -iE 'limit|resource|jetsam|memory' | tr -s ' \n' ' ')"
printf 'StandardOutput=%s\n' "$runner_root/_diag/launchd.stdout.log"
printf 'StandardError=%s\n' "$runner_root/_diag/launchd.stderr.log"
printf 'MemoryAvailableKB=%s\n' "$(vm_stat | awk -F'[:.]' '/page size of/ {size=$0} /Pages free|Pages speculative|Pages purgeable/ {gsub(/[^0-9]/, "", $2); pages+=$2} END {match(size, /[0-9]+/); print pages * substr(size, RSTART, RLENGTH) / 1024}')"
printf 'MemoryTotalKB=%s\n' "$(( $(sysctl -n hw.memsize) / 1024 ))"
printf 'SwapUsage=%s\n' "$(sysctl -n vm.swapusage | tr -s ' ')"
printf 'RunnerAccount=%s\n' "$(id -un "$runner_user" 2>/dev/null || printf 'absent')"
printf 'RunnerRoot=%s\n' "$runner_root"
newest=$(root sh -c "ls -t \"$runner_root\"/_diag/*.log 2>/dev/null | head -n 1" || true)
printf 'HostTime=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf 'LogModifiedAt=%s\n' "$(root sh -c "[ -n \"$newest\" ] && date -u -r \"\$(stat -f %m '$newest')\" +%Y-%m-%dT%H:%M:%SZ" 2>/dev/null || printf 'unknown')"
printf 'WrapperLogModifiedAt=%s\n' "$(date -u -r "$(root stat -f %m "$runner_root/_diag/launchd.stdout.log" 2>/dev/null || printf 0)" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || printf 'unknown')"
listener=$runner_root/bin/Runner.Listener
printf 'ListenerBinary=%s\n' "$(if [ -x "$listener" ]; then printf 'present'; else printf 'absent'; fi)"
printf 'ListenerArchitectures=%s\n' "$(root lipo -archs "$listener" 2>/dev/null || printf 'unreadable')"
printf 'ListenerCodesign=%s\n' "$(root codesign --verify --verbose=1 "$listener" 2>&1 | tr -s ' \n' ' ' || true)"
printf 'ListenerQuarantine=%s\n' "$(root xattr -p com.apple.quarantine "$listener" 2>/dev/null || printf 'none')"
printf 'BundleExtractDir=%s\n' "$(root sh -c "[ -d \"$runner_root/.dotnet\" ] && ls -ld \"$runner_root/.dotnet\" | tr -s ' '" 2>/dev/null || printf 'absent')"
printf 'TemporaryDir=%s\n' "$(root sh -c "[ -d \"$runner_root/.tmp\" ] && ls -ld \"$runner_root/.tmp\" | tr -s ' '" 2>/dev/null || printf 'absent')"
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

/// The memory half of the diagnosis: what the host had, what the unit was
/// allowed, what the fleet declared about reclaiming it, and whether this
/// failure is the runtime refusing to reserve its heap.
///
/// `0x8007000C` is `E_OUTOFMEMORY` and 137 is `SIGKILL`. Both were printed
/// on charless-mac-mini from 2026-09-06 on, and reading them alone sent
/// operators to the runner's registration, its token and its labels — none of
/// which were wrong. The sentence below is the one fact that mattered, and it
/// is stated rather than left to be recognised.
fn memory(head: &str, tail: &str, target: &crate::targets::ComputeTarget) -> Value {
    let kib = |key: &str| -> Option<i64> {
        field(head, key)
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| value as i64)
    };
    let exhausted = tail.contains("0x8007000C") || tail.contains("HRESULT: 0x8007000c");
    let killed = field(head, "ExecMainStatus").trim() == "137";
    let declaration = crate::providers::local::host_memory::schema::declared(target)
        .map(|policy| serde_json::to_value(policy).unwrap_or(Value::Null));
    json!({
        "available_mb": kib("MemoryAvailableKB").map(|value| value / 1024),
        "total_mb": kib("MemoryTotalKB").map(|value| value / 1024),
        "swap": field(head, "SwapUsage").trim(),
        "unit_limits": field(head, "UnitLimits").trim(),
        "runtime_refused_memory": exhausted,
        "killed_by_signal": killed,
        "reclaim": crate::providers::local::host_memory::declaration::policies::automatic_verdict(
            declaration.as_ref(),
        ),
        "detail": if exhausted {
            "the runtime refused to reserve memory (E_OUTOFMEMORY, 0x8007000C); compare the \
             available reading with the unit's own limits below before touching the \
             registration"
        } else if killed {
            "the process was killed by a signal rather than exiting; the readings below are \
             the host's own at the time of this diagnosis"
        } else {
            "no memory refusal is recorded in this log; the readings are the host's own at the \
             time of this diagnosis"
        },
    })
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
        "log": field(head, "DiagnosticLog"),
        "host_time": field(head, "HostTime"),
        "log_modified_at": field(head, "LogModifiedAt"),
        "wrapper_log_modified_at": field(head, "WrapperLogModifiedAt"),
        "standard_output": field(head, "StandardOutput"),
        "standard_error": field(head, "StandardError"),
        "runtime": json!({
            "listener_binary": field(head, "ListenerBinary"),
            "architectures": field(head, "ListenerArchitectures"),
            "codesign": field(head, "ListenerCodesign"),
            "quarantine": field(head, "ListenerQuarantine"),
            "bundle_extract_dir": field(head, "BundleExtractDir"),
            "temporary_dir": field(head, "TemporaryDir"),
        }),
        "memory": memory(head, tail, &target),
        "read": if output.ok() { "complete" } else { "partial" },
        "stderr": output.stderr.trim(),
    }))
}
