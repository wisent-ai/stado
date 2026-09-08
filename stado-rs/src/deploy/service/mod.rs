//! Full service management for registry-managed hosts.
//!
//! NO Python original: `stado/` has no service layer at all, and that
//! absence is the incident this module closes. On the July control-host
//! outage `com.wisent.weles-api` existed on the box and was wedged, but
//! nothing in Stado declared it — so no command could list it, restart it,
//! or even assert that it was supposed to be running.
//! `stado.wisent.com/docs/missing-commands` items seven through fourteen are the
//! resulting gap list; this module is their engine and `cli/service.rs` is
//! their operator surface.
//!
//! Two halves, deliberately kept apart:
//!
//! - **Read side.** [`list_services`] joins the declared managed set
//!   against the latest `host_health/<host>.json` beacons
//!   (`monitor/host_health.rs::load_host_health`). It is beacon-only by
//!   construction and issues no ssh at all, because the moment you most
//!   need to ask "what is supposed to be running here" is the moment the
//!   host has stopped answering.
//! - **Write side.** [`restart_service`], [`sync_service_secret`],
//!   [`check_service_bearer`], [`reset_service_listener`], [`retire_service`],
//!   [`deploy_service`], [`probe_service`], [`tail_logs`] and
//!   [`fetch_unit_file`] ride the shared channel of
//!   `deploy/host_channel.rs` — whose ssh option set is derived from
//!   `deploy/host_reboot.rs::ssh_reboot_argv` rather than re-typed, so
//!   `BatchMode=yes`, `ConnectTimeout` and
//!   `StrictHostKeyChecking=accept-new` cannot drift between the host
//!   commands and the service commands. The remote program is fixed and
//!   narrow, it reports through the same tab-delimited `STADO_*` marker
//!   protocol `deploy/host_recovery.rs::parse_output` established, and
//!   registry data never becomes a shell fragment.
//!
//! The managed set has two sources, and the distinction is load-bearing:
//!
//! - `registry` — declared in the target's `services` array. This is what
//!   [`add_service`] / [`remove_service`] edit, and what
//!   `stado registry doctor` diffs against live host state.
//! - `recovery` — the fixed list `host_recovery::MANAGED_AGENTS` that every
//!   declared `stado` host repair pass restarts. Those units are genuinely
//!   managed, so they are listed, but they are managed by that fixed program
//!   and not by the registry document, so they can be neither adopted nor retired.
//!
//! Unit rendering for [`deploy_service`] is not reimplemented here: it goes
//! through `deploy/local_install.rs::InstallPlan`, the same renderer
//! `stado bootstrap --local` and `stado install-disk-cleanup` use, so a
//! service deployed remotely is byte-identical to one installed locally.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use crate::deploy::local_install::{self, InstallPlan, LocalOs};
use crate::deploy::{
    host_channel, host_recovery, py_str_repr, shlex_quote, CommandOutput, DeployError, Runner,
};
use crate::monitor::host_health::{self, HostHealthError};
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

mod model;
mod observe;
mod ops;
mod remote;

pub use model::*;
pub use observe::*;
pub use ops::*;
pub use remote::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(action: &str, postcondition_met: bool) -> EnsureOutcome {
        EnsureOutcome {
            action: action.to_string(),
            domain: DOMAIN_SYSTEM.to_string(),
            pid: "4242".to_string(),
            path: "/Library/LaunchDaemons/com.wisent.always-on.stado-object-api.plist".to_string(),
            report: RemoteReport {
                postcondition: "unit is loaded and running".to_string(),
                postcondition_state: if postcondition_met {
                    host_channel::POSTCONDITION_MET.to_string()
                } else {
                    "unmet".to_string()
                },
                ..RemoteReport::default()
            },
        }
    }

    /// A converged pass is a success. It was added as one — a drifted unit
    /// file rewritten and kicked in place, without the window `bootout` then
    /// `bootstrap` leaves — and `succeeded()` never admitted it, so the stado
    /// 0.13.11 release submission failed with `could not ensure
    /// com.wisent.always-on.stado-object-api: converged:
    /// /Library/LaunchDaemons/…` after that ensure had done exactly what it
    /// was asked to do.
    #[test]
    fn a_converged_ensure_pass_is_a_success() {
        assert!(outcome(ACTION_CONVERGED, true).succeeded());
        // And it counts as a change, because the host was written to.
        assert!(outcome(ACTION_CONVERGED, true).changed());
    }

    #[test]
    fn every_intended_action_succeeds_only_with_the_postcondition_held() {
        for action in [
            ACTION_CREATED,
            ACTION_RESTARTED,
            ACTION_ALREADY_CORRECT,
            ACTION_CONVERGED,
        ] {
            assert!(outcome(action, true).succeeded(), "{action}");
            assert!(
                !outcome(action, false).succeeded(),
                "{action} must not pass on an unmet postcondition"
            );
        }
    }

    /// Any other word stays a failure the remote program named.
    #[test]
    fn an_unknown_action_is_still_a_failure() {
        assert!(!outcome("exploded", true).succeeded());
        assert!(!outcome("", true).succeeded());
    }

    /// A small file keeps exactly the channel default, so nothing that worked
    /// before changes; a large one gets a budget that grows with its bytes.
    /// The 35 MB Weles worker release that timed out at 138 seconds under the
    /// fixed 120-second clock now gets over four minutes.
    #[test]
    fn the_file_sync_budget_grows_with_the_payload() {
        let floor = host_channel::remote_timeout();
        assert_eq!(sync_timeout(0), floor);
        assert_eq!(sync_timeout(1024), floor);
        // 96 MiB is what `--executable` admits; it must not be admitted with a
        // budget that cannot carry it.
        assert!(sync_timeout(96 * 1024 * 1024) > floor * 3);
        // The real payload: 35_331_163 bytes.
        let real = sync_timeout(35_331_163);
        assert!(real > Duration::from_secs(240), "{real:?}");
    }
}
