//! The platform a runner installs on, the substitutions a declared template
//! takes, and the host-wide job gate both platforms install.

use super::declaration::RunnerProfile;
use crate::deploy::DeployError;
use crate::targets::ComputeTarget;

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
pub(crate) enum Platform {
    LinuxAmd64,
    DarwinArm64,
}

impl Platform {
    pub(crate) fn for_name(name: &str, target_name: &str) -> Result<Self, DeployError> {
        match name {
            "linux-amd64" => Ok(Self::LinuxAmd64),
            "darwin-arm64" => Ok(Self::DarwinArm64),
            other => Err(DeployError(format!(
                "{target_name} declares no supported runner platform for {other:?}; set release_platform in the canonical fleet registry to \"darwin-arm64\" or \"linux-amd64\""
            ))),
        }
    }

    pub(crate) fn for_target(target: &ComputeTarget) -> Result<Self, DeployError> {
        Self::for_name(&target.release_platform, &target.name)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::LinuxAmd64 => "linux-amd64",
            Self::DarwinArm64 => "darwin-arm64",
        }
    }

    pub(crate) fn runner_root(self, profile: &RunnerProfile) -> String {
        match self {
            Self::LinuxAmd64 => format!("/opt/wisent/{}-runner", profile.slug),
            Self::DarwinArm64 => format!("/Users/Shared/{}-runner", profile.slug),
        }
    }

    pub(crate) fn kronika_agent_secret_file(self, profile: &RunnerProfile) -> String {
        format!(
            "{}/.stado/kronika-agent-auth-secret",
            self.runner_root(profile)
        )
    }
}

pub(crate) fn shell_list(values: &[&str]) -> String {
    values.join(", ")
}

pub(crate) fn replace(template: &str, pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .fold(template.to_string(), |text, (marker, value)| {
            text.replace(marker, value)
        })
}

/// Render a platform program from the selected declaration row.
///
/// The local account and root follow `slug`; the service manager's identifier
/// follows `unit_label`. Keeping those substitutions distinct lets a third
/// profile reuse an installer kind without adding a command or hard-coded
/// profile match.
pub(crate) fn profile_template(template: &str, profile: &RunnerProfile) -> String {
    template
        .replace(
            "com.wisent.stado-precheck-runner",
            &format!("com.wisent.{}", profile.unit_label),
        )
        .replace(
            "wisent-stado-precheck-runner.service",
            &format!("wisent-{}.service", profile.unit_label),
        )
        .replace("stado-precheck", &profile.slug)
        .replace("stado_precheck", &profile.slug.replace('-', "_"))
        .replace("__RUNNER_KIND__", &profile.name)
}

/// Where each platform keeps the host's job markers. Fixed, root-created
/// directories with the sticky bit, one marker file per runner account.
pub const LINUX_JOBS_DIR: &str = "/opt/wisent/.stado-runner-jobs";
pub const MACOS_JOBS_DIR: &str = "/Users/Shared/.stado-runner-jobs";

/// One job at a time on this host, across every runner registered on it.
///
/// GitHub gives each runner its own concurrency of one and coordinates nothing
/// between runners, so a machine carrying five of them builds five
/// repositories at once. On 2026-09-06 `charless-mac-mini` did exactly that:
/// free disk fell from 10.6 to 4.9 GiB in twenty minutes, one `git` alone held
/// 1021 MiB resident, free memory reached 597 MiB with 4.7 of 6 GiB of swap in
/// use, and every .NET runner on the box then failed to start with
/// `Failed to create CoreCLR, HRESULT: 0x8007000C`. Nothing GitHub offers
/// bounds that from the host's side.
///
/// The runner runs this before a job's first step
/// (`ACTIONS_RUNNER_HOOK_JOB_STARTED`) and `clean-work.sh` after its last, so
/// a marker written here and removed there spans exactly one job. A waiting
/// runner holds nothing: it waits for the marker to be free, and a marker
/// whose process no longer exists is a crash rather than a running job.
///
/// One text for both platforms and for the test that drives it, because a
/// second copy of a concurrency rule is the copy that gets it wrong.
const JOB_GATE: &str = r#"#!/bin/sh
set -eu
jobs_dir=${STADO_RUNNER_JOBS_DIR:-__JOBS_DIR__}
mine="$jobs_dir/$(id -un).job"
mkdir -p "$jobs_dir" 2>/dev/null || true
chmod 1777 "$jobs_dir" 2>/dev/null || true
deadline=$(( $(date +%s) + ${STADO_RUNNER_JOB_WAIT_SECONDS:-3600} ))
while [ "$(date +%s)" -lt "$deadline" ]; do
  held=""
  for marker in "$jobs_dir"/*.job; do
    [ -f "$marker" ] || continue
    [ "$marker" = "$mine" ] && continue
    pid=$(head -n 1 "$marker" 2>/dev/null || true)
    case "$pid" in
      ''|*[!0-9]*) rm -f "$marker" 2>/dev/null || true; continue ;;
    esac
    if kill -0 "$pid" 2>/dev/null; then held="$marker"; break; fi
    rm -f "$marker" 2>/dev/null || true
  done
  [ -n "$held" ] || break
  printf 'stado: waiting for the job holding this host (%s)\n' "$(basename "$held")"
  sleep "${STADO_RUNNER_JOB_POLL_SECONDS:-10}"
done
printf '%s\n' "${STADO_RUNNER_JOB_PID:-$PPID}" > "$mine"
"#;

/// The gate as it is installed on a host, or as a test runs it.
///
/// `STADO_RUNNER_JOBS_DIR` overrides the compiled directory so a test can
/// drive the exact program a host runs without touching the host's markers.
pub fn job_gate_program(jobs_dir: &str) -> String {
    JOB_GATE.replace("__JOBS_DIR__", jobs_dir)
}
