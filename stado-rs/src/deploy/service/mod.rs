//! Full service management for registry-managed hosts.
//!
//! The registry states which services must run. Observation remains separate
//! from lifecycle operations so a host that cannot be reached still has a
//! readable declaration and last observed state.
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
//!   `BatchMode=yes` and
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
//! `stado bootstrap --local` uses, so a service deployed remotely is
//! byte-identical to one installed locally.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
