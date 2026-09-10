//! The exact installer program a registration renders, per platform.

use crate::deploy::host_precheck_runner::declaration::{runner_profile, RunnerProfile};
use crate::deploy::host_precheck_runner::linux::install::LINUX_INSTALLER;
use crate::deploy::host_precheck_runner::macos::install::MACOS_INSTALLER;
use crate::deploy::host_precheck_runner::macos::runtime::MACOS_RUNTIME_FUNCTIONS;
use crate::deploy::host_precheck_runner::platform::{
    job_gate_program, profile_template, replace, shell_list, Platform, BLOCKED_IPV4_NETWORKS,
    BLOCKED_IPV6_NETWORKS, LINUX_JOBS_DIR, MACOS_JOBS_DIR,
};
use crate::deploy::host_precheck_runner::verdict::scope::{scope_for_profile, RunnerScope};
use crate::deploy::{shlex_quote, DeployError};

pub const PROBIERZ_AGENT_ID: &str = "probierz";
pub const PROBIERZ_AGENT_RESOURCE: &str = "agent:probierz";
pub const RUNNER_VERSION: &str = "2.336.0";
pub const LINUX_SHA256: &str = "04cf0be1aff4c3ec3554466c39124ca250e3effd8873bb7e8d68535aa9505d5d";
pub const MACOS_SHA256: &str = "8e8839c49b7060b6b2154f4931f815df330c27f167d53ef2239ee3dfce28b079";

fn linux_installer(
    target_name: &str,
    registration_token: &str,
    brama_url: &str,
    brama_port: u16,
    decision: Decision,
    profile: &RunnerProfile,
    scope: &RunnerScope,
) -> String {
    let runner_name = format!("{}-{target_name}", profile.slug);
    replace(
        &profile_template(LINUX_INSTALLER, profile),
        &[
            ("__VERSION__", RUNNER_VERSION.to_string()),
            ("__SHA256__", LINUX_SHA256.to_string()),
            ("__TOKEN__", shlex_quote(registration_token)),
            ("__RUNNER_NAME__", shlex_quote(&runner_name)),
            ("__RUNNER_GROUP__", shlex_quote(scope.group(profile))),
            ("__RUNNER_LABELS__", profile.labels_text()),
            (
                "__RESTART_REGISTERED__",
                u8::from(decision.restart_registered).to_string(),
            ),
            (
                "__RECONFIGURE__",
                u8::from(decision.reconfigure).to_string(),
            ),
            ("__REGISTRATION_URL__", scope.registration_url()),
            ("__RUNNER_SCOPE__", shlex_quote(&scope.label())),
            ("__BLOCKED_IPV4__", shell_list(BLOCKED_IPV4_NETWORKS)),
            ("__JOB_GATE__", job_gate_program(LINUX_JOBS_DIR)),
            ("__BRAMA_URL__", shlex_quote(brama_url)),
            ("__KRONIKA_AGENT_ID__", shlex_quote(PROBIERZ_AGENT_ID)),
            ("__BRAMA_PORT__", brama_port.to_string()),
            ("__BLOCKED_IPV6__", shell_list(BLOCKED_IPV6_NETWORKS)),
        ],
    )
}

fn macos_installer(
    target_name: &str,
    registration_token: &str,
    brama_url: &str,
    brama_port: u16,
    decision: Decision,
    profile: &RunnerProfile,
    scope: &RunnerScope,
) -> String {
    let runner_name = format!("{}-{target_name}", profile.slug);
    replace(
        &profile_template(MACOS_INSTALLER, profile),
        &[
            (
                "__MACOS_RUNTIME_FUNCTIONS__",
                MACOS_RUNTIME_FUNCTIONS.to_string(),
            ),
            ("__VERSION__", RUNNER_VERSION.to_string()),
            ("__SHA256__", MACOS_SHA256.to_string()),
            ("__TOKEN__", shlex_quote(registration_token)),
            ("__RUNNER_NAME__", shlex_quote(&runner_name)),
            ("__RUNNER_GROUP__", shlex_quote(scope.group(profile))),
            ("__RUNNER_LABELS__", profile.labels_text()),
            (
                "__RESTART_REGISTERED__",
                u8::from(decision.restart_registered).to_string(),
            ),
            (
                "__RECONFIGURE__",
                u8::from(decision.reconfigure).to_string(),
            ),
            ("__REGISTRATION_URL__", scope.registration_url()),
            ("__RUNNER_SCOPE__", shlex_quote(&scope.label())),
            ("__BRAMA_URL__", shlex_quote(brama_url)),
            ("__KRONIKA_AGENT_ID__", shlex_quote(PROBIERZ_AGENT_ID)),
            ("__BRAMA_PORT__", brama_port.to_string()),
            ("__JOB_GATE__", job_gate_program(MACOS_JOBS_DIR)),
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

/// What one registration names: the declared profile, the host it installs
/// on, the platform whose installer is rendered, and the repository whose
/// scope GitHub is asked for. These four travel together because a scope is
/// only meaningful against the profile that declares it accepts one, and
/// because a rendered installer is reviewable only as the whole set.
pub struct InstallerRequest<'a> {
    pub profile_name: &'a str,
    pub target_name: &'a str,
    pub platform_name: &'a str,
    pub repository: Option<&'a str>,
}

/// What this run has to do beyond installing files, decided by reading the
/// host rather than by guessing from the declaration.
///
/// `reconfigure` is the one that used to be missing. The host programs only
/// registered a runner when none was configured, so an install that moved the
/// scope, the group or the labels wrote files, restarted nothing, registered
/// nothing, and exited 0 — the shape a report cannot distinguish from work.
#[derive(Debug, Clone, Copy, Default)]
pub struct Decision {
    /// GitHub reports this registered runner offline, so the service is cycled.
    pub restart_registered: bool,
    /// The host's own registration record disagrees with what was asked for.
    pub reconfigure: bool,
}

/// Render the exact installer program the host channel will execute.
///
/// Keeping this boundary pure makes registration scope reviewable without a
/// live host while the production path still supplies short-lived credentials.
pub fn installer_program(
    request: &InstallerRequest<'_>,
    registration_token: &str,
    brama_url: &str,
    brama_port: u16,
    decision: Decision,
) -> Result<String, DeployError> {
    let &InstallerRequest {
        profile_name,
        target_name,
        platform_name,
        repository,
    } = request;
    let profile = runner_profile(profile_name)?;
    let scope = scope_for_profile(profile, repository)?;
    let platform = Platform::for_name(platform_name, target_name)?;
    profile.installer_kind(platform.name())?;
    Ok(match platform {
        Platform::LinuxAmd64 => linux_installer(
            target_name,
            registration_token,
            brama_url,
            brama_port,
            decision,
            profile,
            &scope,
        ),
        Platform::DarwinArm64 => macos_installer(
            target_name,
            registration_token,
            brama_url,
            brama_port,
            decision,
            profile,
            &scope,
        ),
    })
}
