//! Registry-authorized recovery for a managed macOS host.
//!
//! The remote program is fixed and deliberately narrow: run the canonical
//! Rust disk cleanup, disable the obsolete local coordinator, and reload the
//! one registry-managed health agent after validating its scoped Skarbiec
//! configuration. Registry data selects only the host; it cannot supply shell
//! fragments.
//!
//! The tab-delimited `STADO_*` marker protocol (script emission in
//! [`remote_script`], parsing in [`parse_output`]) deliberately preserves the
//! mix of literal `\t` / `\n` escape sequences and real control characters
//! consumed by the recovery report parser.
//!
//! The bodies live beside this entry point: `plan` resolves what the pass will
//! act on, `program` is the fixed remote program and its substitutions,
//! `report` folds the marker lines back into the operator's answer, and `run`
//! carries the pass over the host channel.

mod plan;
mod program;
mod report;
mod run;

pub use plan::{plan_agents, plan_stable_binds, AgentPlan, StableBindPlan};
pub use program::{identity_values, remote_script, remote_script_with_stable_binds, ssh_argv};
pub use report::{parse_output, to_sorted_pretty};
pub use run::{recover_host, recover_host_with_registry};

/// Rust Stado cleanup binary. Recovery has no Python-package substitute.
pub const WC_CANDIDATES: &[&str] = &["$HOME/.stado/bin/stado"];

/// The unit every recovery pass reloads, with the plist path to use for a
/// host that declares nothing of its own: the host's one Stado process, which
/// carries host-health publication as a role, as the product catalog names
/// it. Weles lifecycle is owned exclusively by the authenticated Stado
/// service API.
///
/// The path here is the LAST RESORT, not the answer: [`plan_agents`] prefers
/// what the target's `services` array declares. When the two spellings
/// disagree, every pass would report `missing_plist` about a file the host
/// has, so the declaration wins. Empty when the compiled catalog names no host
/// process, which its validation refuses.
pub fn managed_agents() -> Vec<(String, String)> {
    crate::deploy::service_catalog::host_unit()
        .map(|label| {
            let plist = format!("$HOME/Library/LaunchAgents/{label}.plist");
            vec![(label, plist)]
        })
        .unwrap_or_default()
}

/// The pass reloaded the unit and launchd has a job under the label.
pub const AGENT_RESTARTED: &str = "restarted";
/// The pass bootstrapped the unit in the domain the resolver chose and launchd
/// has no job there. Carries launchd's own words after a colon.
///
/// Reported separately from `bootstrap_failed` because the exit status was
/// zero: `launchctl bootstrap` returning success and leaving no job is exactly
/// the case a pass that trusted the exit status called `restarted`.
pub const AGENT_NOT_LOADED: &str = "not_loaded";
/// The declared unit file is not on the host.
pub const AGENT_MISSING_PLIST: &str = "missing_plist";
/// The declared unit file is a system LaunchDaemon; this pass is
/// unprivileged and left it alone.
pub const AGENT_NEEDS_PRIVILEGE: &str = "needs_privileged_bootstrap";

/// Every managed unit ran, nothing was skipped, nothing is blocking.
pub const STATUS_OK: &str = "ok";
/// The pass itself completed, and at least one managed unit was skipped or
/// is blocked. Distinct from `failed`, which is the pass not completing.
pub const STATUS_BLOCKED: &str = "blocked";

/// The stable bind is already served; the pass touched nothing.
pub const STABLE_BIND_ALREADY_BOUND: &str = "already_bound";
/// The legacy daemon was bootstrapped and the port answers now.
pub const STABLE_BIND_RESTORED: &str = "restored";
/// A blue-green candidate is serving, so the release agent owns the handoff
/// and this pass touched nothing. Carries the candidate port after a colon.
pub const STABLE_BIND_CANDIDATE_LIVE: &str = "candidate_live";
