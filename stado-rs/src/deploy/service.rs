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
//!   `stado host recover` pass restarts. Those units are genuinely managed,
//!   so they are listed, but they are managed by that fixed program and not
//!   by the registry document, so they can be neither adopted nor retired.
//!
//! Unit rendering for [`deploy_service`] is not reimplemented here: it goes
//! through `deploy/local_install.rs::InstallPlan`, the same renderer
//! `stado bootstrap --local` and `stado install-disk-cleanup` use, so a
//! service deployed remotely is byte-identical to one installed locally.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, TimeDelta, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::local_install::{self, InstallPlan, LocalOs};
use super::{
    host_channel, host_recovery, py_str_repr, shlex_quote, CommandOutput, DeployError, Runner,
};
use crate::monitor::host_health::{self, HostHealthError};
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

/// Per-target registry key holding the declared service array. Unknown
/// per-target keys land in `targets.rs::ComputeTarget::extra` through
/// `#[serde(flatten)]` and `targets.rs::validate_registry` ignores them, so
/// the array round-trips through the canonical document untouched.
pub const SERVICES_KEY: &str = "services";

/// Declared in the registry document; adopt / retire / deploy edit these.
pub const SOURCE_REGISTRY: &str = "registry";
/// Carried by the fixed `host_recovery::MANAGED_AGENTS` program.
pub const SOURCE_RECOVERY: &str = "recovery";
/// Located by a product declaration
/// ([`crate::deploy::products::Unit`]): the shipped document names the label
/// AND the unit file, which is what makes it addressable without a registry
/// record for it.
pub const SOURCE_PRODUCT: &str = "product";

/// macOS launchd.
pub const KIND_LAUNCHD: &str = "launchd";
/// Linux systemd, in the system or per-user scope.
pub const KIND_SYSTEMD: &str = "systemd";

/// The beacon says the unit is loaded and has not failed.
pub const STATE_ACTIVE: &str = "active";
/// The beacon says the unit is not loaded.
pub const STATE_INACTIVE: &str = "inactive";
/// The beacon says the unit's last exit was non-zero.
pub const STATE_FAILED: &str = "failed";
/// A beacon exists for the host but does not carry this unit at all — the
/// unit is declared here and unaccounted for there.
pub const STATE_MISSING: &str = "missing";
/// Nothing is known: the host has published no beacon, or the beacon
/// carries the unit with an empty state.
pub const STATE_UNKNOWN: &str = "unknown";

/// The `kind` slot of the label [`plan_deploy`] mints, so a deployed
/// service can never collide with the agent / coordinator / disk-cleanup /
/// failure-fixer labels `local_install::label` produces for those kinds.
pub const DEPLOY_KIND: &str = "service";

/// Redaction placeholder. Same spelling `providers/box/types.rs::safe_text`
/// already puts in front of operators.
pub const REDACTED: &str = "[REDACTED]";

/// The launchd domain a unit-file path loads into, decided locally.
///
/// The registry declares paths, and the path alone says which domain the
/// unit lives in — which in turn says whether the approved channel can
/// bootstrap it at all: a system LaunchDaemon loads as root, and the
/// channel is unprivileged. Derived here rather than on the host because
/// the refusal has to happen before the host is contacted.
///
/// This is the local half of [`DOMAIN_RESOLVER`]'s first branch and the two
/// MUST agree on it: `/Library/LaunchDaemons/...` is the system domain here
/// and on the host. Everything the path cannot answer — whether the user has
/// a graphical session, and therefore whether an agent's domain is
/// `gui/<uid>` or the background `user/<uid>` — is the host's answer alone,
/// and this type deliberately does not guess at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitDomain {
    /// `/Library/LaunchDaemons/...` — loads as root.
    System,
    /// `/Library/LaunchAgents/...` — loads for whichever user is logged in.
    AnyUser,
    /// `<home>/Library/LaunchAgents/...` — loads for that user.
    User,
    /// Anything else: a systemd unit path, an empty path.
    Unknown,
}

impl UnitDomain {
    /// Classify one declared unit-file path. The registry's `$HOME/...`
    /// idiom arrives unexpanded, so the user domain is matched on the
    /// `Library/LaunchAgents` segment rather than on a home prefix — which
    /// also covers `/Users/<name>/Library/LaunchAgents/...`.
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/Library/LaunchDaemons/") {
            Self::System
        } else if path.starts_with("/Library/LaunchAgents/") {
            Self::AnyUser
        } else if path.contains("/Library/LaunchAgents/") {
            Self::User
        } else {
            Self::Unknown
        }
    }

    /// True when loading the unit takes root — the system domain only.
    pub fn requires_privileged_bootstrap(&self) -> bool {
        matches!(self, Self::System)
    }

    /// True when the unit's job belongs to a login rather than to the
    /// machine: a LaunchAgent, wherever the plist sits. A host with no
    /// graphical login has no `gui/<uid>` domain to load one into, which is
    /// what makes this the interesting half of the classification for
    /// [`MisdeclaredDomain`].
    pub fn is_per_login(&self) -> bool {
        matches!(self, Self::AnyUser | Self::User)
    }

    /// The `domain` column spelling; empty when the path places no domain.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => DOMAIN_SYSTEM,
            Self::AnyUser => "any-user",
            Self::User => DOMAIN_USER,
            Self::Unknown => "",
        }
    }
}

/// Remote `$HOME` prefix. Registry-declared unit paths use this idiom —
/// `host_recovery::MANAGED_AGENTS` spells every plist that way — so it has
/// to survive into the remote program unexpanded on our side and expanded
/// on theirs.
const HOME_PREFIX: &str = "$HOME";

/// Heredoc delimiter the deploy program uses to carry a rendered unit. The
/// delimiter is quoted in the script, so the remote shell performs no
/// expansion inside the body and the only way out is a body line equal to
/// the delimiter — which [`guard_heredoc`] refuses up front.
const UNIT_HEREDOC: &str = "STADO_UNIT_BODY";

// ---------------------------------------------------------------------------
// The managed set
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingProduct {
    pub product_id: String,
    pub display_name: String,
    pub repository: String,
    pub surface_kinds: Vec<String>,
    pub first_success_fact: String,
    pub onboarding_kind: String,
    pub status: String,
}

/// One unit Stado claims to manage on one host.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagedService {
    /// Registry target name of the host that runs it.
    pub host: String,
    /// Declarative placement selector that resolved to this host.
    pub host_heuristic: Option<String>,
    /// The name the CLI addresses it by.
    pub name: String,
    /// systemd unit name (`foo.service`); empty for a launchd service.
    pub unit: String,
    /// launchd label; empty for a systemd service.
    pub label: String,
    /// Unit-file path on the host, `$HOME`-relative where the declaration
    /// is (as `host_recovery::MANAGED_AGENTS` writes it).
    pub path: String,
    /// [`KIND_LAUNCHD`] or [`KIND_SYSTEMD`].
    pub kind: String,
    /// Absolute program the unit runs, on the host. Present when the
    /// declaration is the source of the unit rather than a pointer at a
    /// plist somebody installed by hand: `service ensure` renders the unit
    /// from this and [`ManagedService::args`], so a host that lost its unit
    /// file can be made to run the right thing again from the document
    /// alone. Empty for a declaration that only names a path.
    pub program: String,
    /// The argument vector [`ManagedService::program`] is started with.
    pub args: Vec<String>,
    /// Non-secret environment rendered into the unit and preserved by repairs.
    pub env: BTreeMap<String, String>,
    /// Exact systemd unit body for a service whose native definition carries
    /// lifecycle semantics the generic renderer cannot express. Empty for the
    /// ordinary generated unit. When present, reconciliation validates that
    /// its `ExecStart` is exactly [`ManagedService::program`] plus
    /// [`ManagedService::args`] before retaining these authored semantics.
    pub systemd_unit: String,
    /// [`SOURCE_REGISTRY`] or [`SOURCE_RECOVERY`].
    pub source: String,
    /// When the unit entered management; empty for a recovery-sourced one,
    /// which has been managed for as long as the program has existed.
    pub managed_since: String,
    /// Product-level onboarding metadata synchronized into Echo.
    pub onboarding: Option<OnboardingProduct>,
}

impl ManagedService {
    /// The host's own name for the unit: the launchd label, or the systemd
    /// unit name. This is what the remote program addresses.
    pub fn unit_id(&self) -> &str {
        if self.label.is_empty() {
            &self.unit
        } else {
            &self.label
        }
    }

    /// True when an operator-supplied NAME addresses this service. Both the
    /// logical name and the host's own name for the unit resolve, so
    /// `service restart weles-api` and
    /// `service restart com.wisent.weles-api` are the same request.
    pub fn matches(&self, query: &str) -> bool {
        self.name == query || self.unit_id() == query
    }

    pub fn to_record(&self) -> Value {
        let mut record = json!({
            "name": self.name,
            "unit": self.unit,
            "label": self.label,
            "path": self.path,
            "kind": self.kind,
            "managed_since": self.managed_since,
        });
        if let Some(heuristic) = &self.host_heuristic {
            record
                .as_object_mut()
                .expect("managed service record")
                .insert(
                    "host_heuristic".to_string(),
                    Value::String(heuristic.clone()),
                );
        }
        // Written only when the declaration actually is the source of the
        // unit. A record that merely points at a path keeps the shape it
        // has always had, so adding this field rewrites no existing entry.
        if !self.program.is_empty() {
            let record = record.as_object_mut().expect("managed service record");
            record.insert("program".to_string(), Value::String(self.program.clone()));
            record.insert(
                "args".to_string(),
                Value::Array(self.args.iter().cloned().map(Value::String).collect()),
            );
        }
        if !self.env.is_empty() {
            record["env"] = json!(self.env);
        }
        if !self.systemd_unit.is_empty() {
            record["systemd_unit"] = Value::String(self.systemd_unit.clone());
        }
        if let Some(onboarding) = &self.onboarding {
            record["onboarding"] =
                serde_json::to_value(onboarding).expect("OnboardingProduct is JSON serializable");
        }
        record
    }

    /// The `--json` rendering: the record plus the resolved host and the
    /// source that declared it.
    pub fn to_json(&self) -> Value {
        let mut record = json!({
            "host": self.host,
            "host_heuristic": self.host_heuristic,
            "name": self.name,
            "unit": self.unit,
            "label": self.label,
            "unit_id": self.unit_id(),
            "path": self.path,
            "kind": self.kind,
            "source": self.source,
            "managed_since": self.managed_since,
            "program": self.program,
            "args": self.args,
            "env": self.env,
            "systemd_unit": self.systemd_unit,
        });
        if let Some(onboarding) = &self.onboarding {
            record["onboarding"] =
                serde_json::to_value(onboarding).expect("OnboardingProduct is JSON serializable");
        }
        record
    }

    /// Read one `services[]` element back. Missing fields read as empty:
    /// the array is operator-facing state in a hand-editable document, and
    /// a half-filled record should degrade to a listed service with blanks
    /// rather than vanish from the managed set.
    fn from_record(host: &str, record: &Map<String, Value>) -> Self {
        let text = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let label = text("label");
        let unit = text("unit");
        let kind = match record.get("kind").and_then(Value::as_str) {
            Some(kind) if !kind.is_empty() => kind.to_string(),
            // Infer from the spelling the record carries, so a record
            // written by hand without a kind still routes to the right
            // remote branch.
            _ if label.is_empty() => KIND_SYSTEMD.to_string(),
            _ => KIND_LAUNCHD.to_string(),
        };
        let name = match text("name") {
            name if !name.is_empty() => name,
            _ if label.is_empty() => unit.clone(),
            _ => label.clone(),
        };
        Self {
            host: host.to_string(),
            host_heuristic: record
                .get("host_heuristic")
                .and_then(Value::as_str)
                .map(str::to_string),
            name,
            unit,
            label,
            path: text("path"),
            kind,
            source: SOURCE_REGISTRY.to_string(),
            managed_since: text("managed_since"),
            program: text("program"),
            args: record
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            env: record
                .get("env")
                .and_then(Value::as_object)
                .map(|env| {
                    env.iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            systemd_unit: text("systemd_unit"),
            onboarding: record
                .get("onboarding")
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
        }
    }
}

/// A launchd-managed service, the shape both the recovery agents and an
/// adopted macOS unit take.
pub fn launchd_service(
    host: &str,
    label: &str,
    path: &str,
    source: &str,
    since: &str,
) -> ManagedService {
    ManagedService {
        host: host.to_string(),
        host_heuristic: None,
        name: label.to_string(),
        unit: String::new(),
        label: label.to_string(),
        path: path.to_string(),
        kind: KIND_LAUNCHD.to_string(),
        source: source.to_string(),
        managed_since: since.to_string(),
        program: String::new(),
        args: Vec::new(),
        env: BTreeMap::new(),
        systemd_unit: String::new(),
        onboarding: None,
    }
}

/// A systemd-managed service, in either the system or per-user scope.
pub fn systemd_service(
    host: &str,
    unit: &str,
    path: &str,
    source: &str,
    since: &str,
) -> ManagedService {
    ManagedService {
        host: host.to_string(),
        host_heuristic: None,
        name: unit.to_string(),
        unit: unit.to_string(),
        label: String::new(),
        path: path.to_string(),
        kind: KIND_SYSTEMD.to_string(),
        source: source.to_string(),
        managed_since: since.to_string(),
        program: String::new(),
        args: Vec::new(),
        env: BTreeMap::new(),
        systemd_unit: String::new(),
        onboarding: None,
    }
}

/// Every unit Stado manages on one target: the registry-declared array
/// first, then macOS recovery agents on hosts declared to run macOS. A
/// declaration wins over the fixed list, because an operator who adopted a
/// recovery label explicitly said what its path and name are.
pub fn declared_services(target: &ComputeTarget) -> Vec<ManagedService> {
    let mut services: Vec<ManagedService> = target
        .extra
        .get(SERVICES_KEY)
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .filter_map(Value::as_object)
                // `service declare` records desired state before a unit
                // exists. Operational commands must not address that
                // placeholder as a loaded or managed unit; `deploy` replaces
                // it through `record_declaration` after the host action.
                .filter(|record| record.get("declared_only").and_then(Value::as_bool) != Some(true))
                .map(|record| ManagedService::from_record(&target.name, record))
                .collect()
        })
        .unwrap_or_default();
    if !crate::targets::platform_accepts_job(&target.release_platform, "Darwin", "") {
        return services;
    }
    for (label, plist) in host_recovery::MANAGED_AGENTS {
        if services.iter().any(|service| service.matches(label)) {
            continue;
        }
        services.push(launchd_service(
            &target.name,
            label,
            plist,
            SOURCE_RECOVERY,
            "",
        ));
    }
    services
}

// ---------------------------------------------------------------------------
// Declared domain against the domain the host can have
// ---------------------------------------------------------------------------

/// The `role` / `host_heuristic` word for a host that is expected to serve
/// with nobody sitting at it. `control-host` carries it in both fields.
pub const ROLE_ALWAYS_ON: &str = "always-on";

/// Does the registry say this host is meant to keep a graphical account alive?
///
/// `always-on` describes uptime, not the absence of a login. The Mac mini is
/// both always-on and the declared Weles host; autologin keeps its Aqua domain
/// alive so browser-facing LaunchAgents can run there. Treating uptime as
/// headlessness moved those jobs into the system domain, where they competed
/// with the release-owned user jobs for the same ports.
pub fn declared_graphical(target: &ComputeTarget) -> bool {
    target.weles.as_ref().is_some_and(|policy| policy.enabled) || target.display_stream.is_some()
}

/// Does the registry itself say this host runs unattended?
pub fn declared_always_on(target: &ComputeTarget) -> bool {
    [target.role.as_deref(), target.host_heuristic.as_deref()]
        .into_iter()
        .flatten()
        .any(|word| word == ROLE_ALWAYS_ON)
}

/// Is the system domain the only declared launchd domain for this host?
///
/// An always-on Darwin target defaults to system services only when the same
/// declaration does not assign it a persistent graphical workload. Linux uses
/// systemd user lingering and does not have launchd domains.
pub fn requires_daemon_domain(target: &ComputeTarget) -> bool {
    declared_always_on(target)
        && !declared_graphical(target)
        && !target.release_platform.starts_with("linux")
}

/// Where a launchd job that belongs to the machine lives.
const DAEMON_DIR: &str = "/Library/LaunchDaemons";
/// The `/Users/<account>/...` prefix a per-account agent path carries. The
/// account is load-bearing: a LaunchAgent's job runs as its owner, and the
/// daemon spelling of the same unit only keeps running as that owner if it
/// carries `UserName` (`local_install::daemon_plist_text`).
const ACCOUNTS_PREFIX: &str = "/Users/";

/// A unit declared in a launchd domain the host it is declared on cannot
/// have.
///
/// `com.wisent.compute.service.stado-agent-mini` was declared as a user
/// LaunchAgent at `/Users/charles/Library/LaunchAgents/...` on
/// `control-host`, a host declared always-on in both `role` and
/// `host_heuristic` and with no graphical session at all: `/dev/console` is
/// root's, `who` prints nothing, and the login's own `launchctl list` holds
/// no `com.wisent.*` label. `launchctl bootstrap user/501 <plist>` answers
/// `Bootstrap failed: 5: Input/output error` there and `gui/501` does not
/// exist, so the declaration named a domain that could never load it. Every
/// other always-on unit on that host is a system LaunchDaemon under
/// [`DAEMON_DIR`].
///
/// The declaration is checkable without going anywhere: the path says the
/// domain and the target says the host runs unattended. So this is a
/// registry finding, reported by `stado registry doctor` and printed under
/// `stado service list`, rather than a surprise the next `restart` produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MisdeclaredDomain {
    /// Registry target the unit is declared on.
    pub host: String,
    /// The host's own name for the unit — the launchd label.
    pub unit: String,
    /// The unit-file path the declaration carries.
    pub path: String,
    /// The domain that path places the unit in, as [`UnitDomain::as_str`]
    /// spells it.
    pub declared_domain: &'static str,
    /// The only domain this host can load a unit into.
    pub loadable_domain: &'static str,
    /// Where the daemon spelling of this unit belongs.
    pub daemon_path: String,
    /// The account the agent's job runs as, read out of the declared path;
    /// empty for a machine-wide `/Library/LaunchAgents` declaration, which
    /// names no account at all.
    pub account: String,
}

impl MisdeclaredDomain {
    /// The finding for one declared unit, or `None` when the declaration and
    /// the host agree.
    ///
    /// Registry-declared units only. A `host_recovery::MANAGED_AGENTS` entry
    /// is carried by that fixed program and not by the document, so it is
    /// not a registry finding and correcting the document would not move it.
    pub fn detect(target: &ComputeTarget, service: &ManagedService) -> Option<Self> {
        if service.source != SOURCE_REGISTRY || !requires_daemon_domain(target) {
            return None;
        }
        let declared = UnitDomain::from_path(&service.path);
        if !declared.is_per_login() {
            return None;
        }
        let unit = service.unit_id().to_string();
        let account = service
            .path
            .strip_prefix(ACCOUNTS_PREFIX)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_default()
            .to_string();
        Some(Self {
            host: target.name.clone(),
            daemon_path: format!("{DAEMON_DIR}/{unit}.plist"),
            unit,
            path: service.path.clone(),
            declared_domain: declared.as_str(),
            loadable_domain: DOMAIN_SYSTEM,
            account,
        })
    }

    /// The privileged command that puts this unit in the domain the host can
    /// load, spelled the way `ENSURE_BODY` installs a daemon
    /// (`install -m 644 -o root -g wheel`) so the file an operator writes by
    /// hand and the file the fleet writes have the same owner and mode.
    ///
    /// `UserName` rides along wherever the declared path names an account:
    /// root reads a plist in [`DAEMON_DIR`], and a daemon without that key
    /// would run the account's program as uid 0 against an account-owned
    /// `~/.stado` — the exact trade `local_install::daemon_plist_text`
    /// documents.
    pub fn install_command(&self) -> String {
        let install = format!(
            "/usr/bin/install -m 644 -o root -g wheel {} {}",
            self.path, self.daemon_path
        );
        if self.account.is_empty() {
            return format!("sudo {install}");
        }
        format!(
            "sudo /bin/sh -c '{install} && /usr/bin/plutil -insert UserName -string {} {}'",
            self.account, self.daemon_path
        )
    }

    /// The one sentence both surfaces print: the unit, the domain it
    /// declares, the domain the host can actually load, and the command that
    /// closes the gap.
    pub fn sentence(&self) -> String {
        format!(
            "{} is declared in launchd's {} domain ({}), and {} is declared {ROLE_ALWAYS_ON}, so no \
             account is logged in graphically there, launchd builds no gui/<uid>, and {} is the only \
             domain that host can load a unit into; install it there with one privileged command on \
             the host: {}",
            self.unit,
            self.declared_domain,
            self.path,
            self.host,
            self.loadable_domain,
            self.install_command()
        )
    }

    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "unit": self.unit,
            "path": self.path,
            "declared_domain": self.declared_domain,
            "loadable_domain": self.loadable_domain,
            "daemon_path": self.daemon_path,
            "install_command": self.install_command(),
            "detail": self.sentence(),
        })
    }
}

/// Every registry-declared unit on TARGET whose declared launchd domain the
/// host cannot have.
pub fn misdeclared_domains(target: &ComputeTarget) -> Vec<MisdeclaredDomain> {
    declared_services(target)
        .iter()
        .filter_map(|service| MisdeclaredDomain::detect(target, service))
        .collect()
}

/// A unit delivered through the compiled managed-product catalog whose host
/// declares no desired version for that product.
///
/// Stado has two independent delivery mechanisms:
///
/// - [`crate::release_control`] owns the desired version of blue-green and
///   replace releases in `release_control.products.<product>.desired`;
/// - [`crate::deploy::products`] owns `host release`, whose per-host desired
///   versions live in `targets[].managed_versions`.
///
/// Requiring both declarations for one product creates two authorities and,
/// for release-control-only products such as Brama, recommends a
/// `host declare-version` command the compiled managed-product catalog refuses.
/// This row therefore exists only for units that map to that compiled catalog
/// and are not owned by release control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredServiceVersion {
    pub host: String,
    /// The exact compiled product name accepted by `host declare-version`.
    pub product: String,
    pub name: String,
    pub unit: String,
    pub program: String,
}

impl UndeclaredServiceVersion {
    pub fn sentence(&self) -> String {
        format!(
            "{} ({}) runs {} from Stado's delivery tree as managed product {:?}, but {} \
             declares no version for it. Declare one with `stado host declare-version {} \
             --binary {} --version X.Y.Z`; that command is valid because {} is present in {}",
            self.name,
            self.unit,
            self.program,
            self.product,
            self.host,
            self.host,
            self.product,
            self.product,
            crate::deploy::products::DECLARATION_PATH,
        )
    }

    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "product": self.product,
            "name": self.name,
            "unit": self.unit,
            "program": self.program,
            "evidence": {"kind": "managed-delivery-unit"},
            "detail": self.sentence(),
        })
    }
}

/// The delivery-tree segment a program path sits under, when it is one.
fn delivery_tree_product(program: &str) -> Option<&str> {
    let (_, tail) = program.split_once("/.stado/services/")?;
    let product = tail.split('/').next()?;
    if product.is_empty() || !tail.contains('/') {
        return None;
    }
    Some(product)
}
/// Whether one registry unit executes Stado from its independently installed
/// service tree rather than from the host-global `$HOME/.stado/bin/stado`.
///
/// The path shape is the same declaration [`delivery_tree_product`] already
/// uses for release inventory. Reusing it keeps release selection and reader
/// convergence from inventing two meanings of "service-local".
pub fn is_service_local_stado_reader(service: &ManagedService) -> bool {
    delivery_tree_product(&service.program).is_some()
        && service.program.rsplit('/').next() == Some("stado")
}

/// Resolve a delivery-tree unit to the exact product name the compiled
/// `host release` catalog accepts.
///
/// Most units are staged under that product name. A few service-specific trees
/// run a catalog binary (for example a control-plane unit staged under its
/// label but executing `stado`), so the program file name is the second
/// supported witness. An arbitrary tree installed by `service update` is not a
/// managed-product declaration and gets no invented semver contract.
fn managed_product_name(delivery_product: &str, program: &str) -> Option<String> {
    let file_name = program.rsplit('/').next().unwrap_or_default();
    let products = crate::deploy::products::declared().ok()?;
    products
        .iter()
        .find(|entry| entry.name == delivery_product || entry.name == file_name)
        .map(|entry| entry.name.clone())
}

/// Every label `stop_legacy` boots out on TARGET, across every product the
/// rollout policy declares for it.
///
/// A unit in this set is scheduled for bootout, not for service liveness or
/// managed-product delivery, so both doctor checks use this one answer.
pub(crate) fn legacy_launchd_labels(
    target: &ComputeTarget,
    control: Option<&crate::release_control::ReleaseControl>,
) -> BTreeSet<String> {
    control
        .into_iter()
        .flat_map(|control| control.products.values())
        .filter_map(|policy| policy.targets.get(&target.name))
        .filter_map(|policy_target| policy_target.legacy_launchd_label.clone())
        .collect()
}

/// Every release-control product targeting this host.
fn release_control_products(
    target: &ComputeTarget,
    control: Option<&crate::release_control::ReleaseControl>,
) -> BTreeSet<String> {
    control
        .into_iter()
        .flat_map(|control| &control.products)
        .filter(|(_, policy)| policy.targets.contains_key(&target.name))
        .map(|(product, _)| product.clone())
        .collect()
}

/// Every compiled managed-product unit on TARGET whose host declares no
/// `managed_versions` entry.
pub fn managed_units_without_declared_version(
    target: &ComputeTarget,
    control: Option<&crate::release_control::ReleaseControl>,
) -> Vec<UndeclaredServiceVersion> {
    let legacy_labels = legacy_launchd_labels(target, control);
    let release_products = release_control_products(target, control);
    let mut rows: BTreeMap<String, UndeclaredServiceVersion> = BTreeMap::new();

    for service in declared_services(target) {
        if legacy_labels.contains(service.unit_id()) {
            continue;
        }
        let Some(delivery_product) = delivery_tree_product(&service.program) else {
            continue;
        };
        if release_products.contains(delivery_product) {
            continue;
        }
        let Some(product) = managed_product_name(delivery_product, &service.program) else {
            continue;
        };
        if target.declared_version(&product).is_some() {
            continue;
        }
        rows.entry(product.clone())
            .or_insert_with(|| UndeclaredServiceVersion {
                host: target.name.clone(),
                product,
                name: service.name.clone(),
                unit: service.unit_id().to_string(),
                program: service.program.clone(),
            });
    }

    rows.into_values().collect()
}

/// The unit file on THIS machine, as the machine holds it.
///
/// A plain local read and never ssh, which is what makes it usable from
/// `registry doctor`: that command answers for the whole fleet out of the
/// store, and the one host whose unit files it may open is the one it is
/// running on. A unit on another host yields `None`, and the finding says
/// the read did not happen instead of reporting an empty environment —
/// "nothing was read" and "the unit carries nothing" are different facts,
/// and collapsing them is the exact defect this check exists to catch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalUnitFile {
    /// Variable names the unit file carries, in file order. Names only: the
    /// finding is about which variables reach the unit, and a diagnostic has
    /// no business printing the values of the ones that do.
    pub carries: Vec<String>,
    /// Values retained only for comparison; diagnostic sentences print names.
    pub env: BTreeMap<String, String>,
    /// The first [`LocalUnitFile::arguments`] entry: the file the unit
    /// declares it starts. Empty for a systemd unit, whose `ExecStart` this
    /// reader does not parse.
    pub program: String,
    /// The whole `ProgramArguments` vector, empty for a systemd unit.
    ///
    /// The whole vector and not just the program, because that is what
    /// launchd execs and therefore what a process table shows: every stado
    /// unit on a host runs the same binary, so `argv[0]` alone cannot tell
    /// the coordinator's process from the agent's from the janitor's, and
    /// [`units_running_replaced_images`] joins on the vector for exactly
    /// that reason.
    pub arguments: Vec<String>,
}

/// Read one unit file off the local filesystem, when it is there to read.
///
/// Every failure — absent, unreadable, a binary plist, unparsable — is the
/// same `None`: the caller's sentence then states that this host's unit was
/// not read, which is true of all of them.
pub fn local_unit_file(path: &str, kind: &str) -> Option<LocalUnitFile> {
    let text = std::fs::read_to_string(path).ok()?;
    if kind == KIND_LAUNCHD {
        let document = parse_plist(&text).ok()?;
        // `Program` as well as `ProgramArguments`: a plist may carry either,
        // and `self_update::launchd_argv` already falls back the same way.
        let arguments: Vec<String> = document
            .get("ProgramArguments")
            .and_then(Value::as_array)
            .map(|argv| {
                argv.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .filter(|argv: &Vec<String>| !argv.is_empty())
            .or_else(|| {
                document
                    .get("Program")
                    .and_then(Value::as_str)
                    .map(|program| vec![program.to_string()])
            })
            .unwrap_or_default();
        let env: BTreeMap<String, String> = plist_env(&document).into_iter().collect();
        Some(LocalUnitFile {
            carries: env.keys().cloned().collect(),
            env,
            program: arguments.first().cloned().unwrap_or_default(),
            arguments,
        })
    } else {
        let parsed = parse_systemd_unit(&text);
        Some(LocalUnitFile {
            carries: parsed.env.iter().map(|(name, _)| name.clone()).collect(),
            env: parsed.env.into_iter().collect(),
            program: String::new(),
            arguments: Vec::new(),
        })
    }
}

/// Why a product's declared environment does not reach the unit serving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentGap {
    /// The registry adopted the unit as a pointer at a plist somebody
    /// installed by hand: it records no program, no arguments and no
    /// environment, so the document states nothing about what the unit
    /// starts with. [`ManagedService::program`] documents that shape
    /// already — "empty for a declaration that only names a path" — and an
    /// adopted stub is the one case where the registry's own record cannot
    /// be diffed against anything.
    UnrecordedDeclaration {
        /// `managed_since`: how long the stub has stood.
        adopted_at: String,
        /// The unit file as this machine holds it, or `None` when the unit
        /// is on another host and was therefore not read.
        observed: Option<LocalUnitFile>,
    },
    /// This product has no release target for the host and its required
    /// environment is not pinned in the managed service declaration.
    HostNamedByNoTarget {
        /// The hosts this product's `targets` map does name, so the row says
        /// where the declaration does land.
        named_hosts: Vec<String>,
    },
    /// The service records the required values, but its native definition
    /// either disagrees or could not be read on this host.
    PinnedServiceEnvironment { observed: Option<LocalUnitFile> },
}

/// One product whose declared environment cannot reach the unit that serves
/// it on one host.
///
/// Measured on `lukasz-macbook` on 2026-09-02, and the measurement is what
/// fixed this check's shape. `release_control.products.skarbiec.environment`
/// declares `SKARBIEC_AUDIT_FILE` and `SKARBIEC_VAULT_FILE`; that product's
/// `targets` map names `charless-mac-mini` only, and in fact no product
/// names `lukasz-macbook` at all. The host nevertheless declares
/// `managed_versions.skarbiec` and runs
/// `com.wisent.compute.service.skarbiec-control-plane`, which the registry
/// adopted as inventory on 2026-09-01 with no program, no args and no
/// environment recorded — three weeks after the plist was hand-created. Its
/// `EnvironmentVariables` is an empty dict and its only `ProgramArguments`
/// entry is a hand-authored launcher that exports `SKARBIEC_VAULT_FILE` and
/// never mentions `SKARBIEC_AUDIT_FILE`, so the journal went to the
/// unpinned default and reached 573,321,978 bytes while the sibling unit
/// that pins it held 34,486,246.
///
/// Nothing reported any of it. Every existing check either validated the
/// declaration's own syntax or compared it against another declaration:
/// `declared_units` (`cli/registry.rs:935`) reads a record's label and
/// nothing else, the beacon publishes one `state` word per unit
/// (`deploy/host_health_beacon_macos.sh:108`), and the only comparison of a
/// product against a host asks `policy.targets.get(host)` first, so the
/// host missing from every target map is the loop's skip condition rather
/// than its finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreachableProductEnvironment {
    pub host: String,
    /// `release_control` product key whose policy declares the environment.
    pub product: String,
    /// Variables that policy declares, with `{home}` expanded exactly as
    /// `release_agent::spawn_release` expands it — from this host's own
    /// `ReleaseTargetPolicy::home`, the only home the fleet declares. A
    /// host no product target names has no declared home, so its row prints
    /// the template verbatim, which is precisely the declaration that
    /// reaches nothing.
    pub declared: Vec<(String, String)>,
    /// The unit an operator can address on this host.
    pub unit: String,
    /// Unit-file path as the registry records it.
    pub path: String,
    pub gap: EnvironmentGap,
}

impl UnreachableProductEnvironment {
    /// Stable machine-readable category, in `registry doctor`'s vocabulary.
    pub fn kind(&self) -> &'static str {
        match self.gap {
            EnvironmentGap::UnrecordedDeclaration { .. } => "unrecorded-service-environment",
            EnvironmentGap::HostNamedByNoTarget { .. } => "untargeted-product-host",
            EnvironmentGap::PinnedServiceEnvironment { observed: Some(_) } => {
                "service-environment-drift"
            }
            EnvironmentGap::PinnedServiceEnvironment { observed: None } => {
                "unread-service-environment"
            }
        }
    }

    /// Names the product declares, for the half of the sentence every
    /// variant shares.
    fn declared_names(&self) -> Vec<&str> {
        self.declared
            .iter()
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Declared `name=value` pairs, which is the spelling that tells an
    /// operator what the unit was supposed to hold.
    fn declared_pairs(&self) -> String {
        self.declared
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<String>>()
            .join(", ")
    }

    pub fn sentence(&self) -> String {
        match &self.gap {
            EnvironmentGap::UnrecordedDeclaration {
                adopted_at,
                observed,
            } => {
                let mut sentence = format!(
                    "registry adopted {} on {} as inventory{}, recording no program, no \
                     arguments and no environment, so the document states nothing about what \
                     that unit starts with — while release_control.products.{}.environment \
                     declares {}",
                    self.unit,
                    self.host,
                    if adopted_at.is_empty() {
                        String::new()
                    } else {
                        format!(" on {adopted_at}")
                    },
                    self.product,
                    self.declared_pairs(),
                );
                match observed {
                    Some(unit) => {
                        let missing: Vec<&str> = self
                            .declared_names()
                            .into_iter()
                            .filter(|name| !unit.carries.iter().any(|held| held == name))
                            .collect();
                        sentence.push_str(&format!(
                            ". The unit file {} on this host carries {}",
                            self.path,
                            if unit.carries.is_empty() {
                                "no environment variables at all".to_string()
                            } else {
                                unit.carries.join(", ")
                            },
                        ));
                        if !unit.program.is_empty() {
                            sentence.push_str(&format!(", and runs {}", unit.program));
                        }
                        if missing.is_empty() {
                            sentence.push_str(
                                ". Every declared variable is present, so only the record is \
                                 missing: nothing in the document can confirm that, and the \
                                 next hand-edit of the unit will go unreported",
                            );
                        } else {
                            sentence.push_str(&format!(
                                ". {} declared and the unit does not carry {}, so whatever \
                                 reads {} falls back to its own default with nothing recording \
                                 that it did",
                                if missing.len() == 1 {
                                    format!("{} is", missing[0])
                                } else {
                                    format!("{} are", missing.join(" and "))
                                },
                                if missing.len() == 1 { "it" } else { "them" },
                                if missing.len() == 1 { "it" } else { "them" },
                            ));
                        }
                    }
                    None => sentence.push_str(&format!(
                        ". The unit file {} was not read: {} is not the host this ran on, and \
                         `registry doctor` answers from the store and never sshes. Read what \
                         it carries with `stado service env {} {}`",
                        self.path, self.host, self.host, self.unit
                    )),
                }
                sentence.push_str(&format!(
                    ". Record what the unit runs and carries with `stado service adopt {} {}` \
                     so a later read has something to disagree with",
                    self.host, self.unit
                ));
                sentence
            }
            EnvironmentGap::PinnedServiceEnvironment { observed } => match observed {
                Some(unit) => {
                    let differences = self
                        .declared
                        .iter()
                        .filter_map(|(name, expected)| {
                            let actual = unit.env.get(name);
                            (actual != Some(expected)).then(|| {
                                format!(
                                    "{name}: expected {expected}, observed {}",
                                    actual.map(String::as_str).unwrap_or("<absent>")
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!(
                        "{} pins {} in the service record for {}, but its native unit {} \
                         differs: {}. Reconcile that service with `stado service ensure {} \
                         --host {}`",
                        self.host,
                        self.declared_pairs(),
                        self.unit,
                        self.path,
                        differences,
                        self.unit,
                        self.host
                    )
                }
                None => format!(
                    "{} pins {} in the service record for {}, so a delivery path exists, \
                     but the native unit {} could not be read here. Its environment is \
                     unverified; run `stado registry doctor` on {}",
                    self.host,
                    self.declared_pairs(),
                    self.unit,
                    self.path,
                    self.host
                ),
            },
            EnvironmentGap::HostNamedByNoTarget { named_hosts } => {
                let mut sentence = format!(
                    "{} declares managed_versions.{} and runs {}, but this product's \
                     release_control target map does not name {}",
                    self.host, self.product, self.unit, self.host,
                );
                if named_hosts.is_empty() {
                    sentence.push_str(&format!(
                        "; release_control.products.{}.targets is empty",
                        self.product
                    ));
                } else {
                    sentence.push_str(&format!(
                        "; release_control.products.{}.targets names {} instead",
                        self.product,
                        named_hosts.join(" and ")
                    ));
                }
                sentence.push_str(&format!(
                    ". The release agent applies products.{}.environment only for a host that \
                     map names, so {} cannot reach {} by any delivery path, and every product \
                     check here resolves the same targets entry first and so skips this host \
                     rather than reporting it. Either name {} in \
                     release_control.products.{}.targets, or pin {} in {} on this host",
                    self.product,
                    self.declared_pairs(),
                    self.host,
                    self.host,
                    self.product,
                    self.declared_names().join(" and "),
                    self.path,
                ));
                sentence
            }
        }
    }

    pub fn to_json(&self) -> Value {
        let declared: Map<String, Value> = self
            .declared
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect();
        let gap = match &self.gap {
            EnvironmentGap::UnrecordedDeclaration {
                adopted_at,
                observed,
            } => json!({
                "kind": "unrecorded-declaration",
                "adopted_at": adopted_at,
                "unit_carries": observed.as_ref().map(|unit| unit.carries.clone()),
                "unit_program": observed.as_ref().map(|unit| unit.program.clone()),
            }),
            EnvironmentGap::HostNamedByNoTarget { named_hosts } => json!({
                "kind": "host-named-by-no-target",
                "named_hosts": named_hosts,
            }),
            EnvironmentGap::PinnedServiceEnvironment { observed } => json!({
                "kind": "pinned-service-environment",
                "observed": observed.as_ref().map(|unit| {
                    self.declared.iter().map(|(name, _)| {
                        (name.clone(), json!(unit.env.get(name)))
                    }).collect::<Map<String, Value>>()
                }),
            }),
        };
        json!({
            "host": self.host,
            "product": self.product,
            "declared": declared,
            "unit": self.unit,
            "path": self.path,
            "gap": gap,
        })
    }
}

/// True when PRODUCT is named as a whole delimited run of UNIT's identifier.
///
/// Launchd labels are dot- and dash-delimited
/// (`com.wisent.compute.service.skarbiec-control-plane`), so the product a
/// unit serves is a delimited segment of its label rather than a substring
/// of it. Both sides are canonicalised to one delimiter and fenced with it,
/// which is what lets a product whose own name carries a delimiter match:
/// half the products in this fleet are spelled `weles-worker` or
/// `image-video-router`, and comparing single tokens would have matched
/// none of them — a check that silently matches nothing is the defect this
/// one was written to catch.
///
/// Fenced and not `contains`: a bare `contains` reads `skarbiec` out of
/// `com.wisent.skarbiecd`, and a check that fires on a coincidence is a
/// check an operator turns off.
fn unit_names_product(unit: &str, product: &str) -> bool {
    if product.is_empty() {
        return false;
    }
    fn fenced(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('.');
        for character in text.chars() {
            out.push(match character {
                '.' | '-' | '_' | '/' => '.',
                other => other.to_ascii_lowercase(),
            });
        }
        out.push('.');
        out
    }
    fenced(unit).contains(&fenced(product))
}

/// Every product whose declared environment cannot reach the unit serving it
/// on TARGET.
///
/// Two conditions, one instrument, because they are one root cause read at
/// two ranges: the product's `environment` is written by exactly one place
/// (`release_agent::spawn_release`) and reached through exactly one lookup
/// (`policy.targets.get(host)`), so a host that lookup misses gets no
/// environment, and a unit the registry adopted as a bare path records none
/// either. Both are reported against the same product policy and in the same
/// words.
///
/// Both require two independent witnesses that this host really runs the
/// product — the host's own `managed_versions` entry, which is what every
/// version diagnostic here enumerates, and an adopted unit whose identifier
/// names the product. Requiring both is deliberate: a product declares an
/// environment on every host in this fleet, so keying off the policy alone
/// would fire on every host that has one, and a check that fires everywhere
/// is a check that gets switched off with the defect still in place.
///
/// `local_units` decides which unit files may be opened: it is the name of
/// the host this process is running on, when the registry resolves one.
/// Every other host's unit is reported unread rather than empty.
pub fn unreachable_product_environments(
    target: &ComputeTarget,
    control: Option<&crate::release_control::ReleaseControl>,
    local_units: Option<&str>,
) -> Vec<UnreachableProductEnvironment> {
    let Some(control) = control else {
        return Vec::new();
    };
    let readable_here = local_units == Some(target.name.as_str());
    let services = declared_services(target);
    let mut rows: Vec<UnreachableProductEnvironment> = Vec::new();
    for (product, policy) in &control.products {
        if policy.environment.is_empty() {
            continue;
        }
        // The host's own statement that it runs this product. `stado host
        // declare-version` writes it and `host reconcile` reads it, so it is
        // the fleet's existing answer to "does this box run that product".
        if target.declared_version(product).is_none() {
            continue;
        }
        let Some(service) = services
            .iter()
            .find(|service| unit_names_product(service.unit_id(), product))
        else {
            continue;
        };
        // Use the same home expansion as the delivery path that owns the values.
        let home = policy
            .targets
            .get(&target.name)
            .map(|policy_target| policy_target.home.clone())
            .unwrap_or_else(|| crate::deploy::service_catalog::home_for(target));
        let declared: Vec<(String, String)> = policy
            .environment
            .iter()
            .map(|(name, value)| {
                let value = if home.is_empty() {
                    value.clone()
                } else {
                    value.replace("{home}", &home)
                };
                (name.clone(), value)
            })
            .collect();
        let row = |gap: EnvironmentGap| UnreachableProductEnvironment {
            host: target.name.clone(),
            product: product.clone(),
            declared: declared.clone(),
            unit: service.unit_id().to_string(),
            path: service.path.clone(),
            gap,
        };
        if !policy.targets.contains_key(&target.name) {
            let pinned_in_service = declared
                .iter()
                .all(|(name, value)| service.env.get(name) == Some(value));
            if pinned_in_service {
                let observed = readable_here
                    .then(|| local_unit_file(&service.path, &service.kind))
                    .flatten();
                let agrees = observed.as_ref().is_some_and(|unit| {
                    declared
                        .iter()
                        .all(|(name, value)| unit.env.get(name) == Some(value))
                });
                if !agrees {
                    rows.push(row(EnvironmentGap::PinnedServiceEnvironment { observed }));
                }
            } else {
                let mut named_hosts: Vec<String> = policy.targets.keys().cloned().collect();
                named_hosts.sort();
                rows.push(row(EnvironmentGap::HostNamedByNoTarget { named_hosts }));
            }
        }
        // An adopted stub: the record names a path and declares nothing about
        // what runs there. Reported independently of the target question,
        // because a host the policy DOES name still has no recorded
        // declaration to diff, and that is the second silence.
        if service.program.is_empty() {
            rows.push(row(EnvironmentGap::UnrecordedDeclaration {
                adopted_at: service.managed_since.clone(),
                observed: readable_here
                    .then(|| local_unit_file(&service.path, &service.kind))
                    .flatten(),
            }));
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// Read side: the beacon join
// ---------------------------------------------------------------------------

/// A managed unit with the state the latest beacon reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub service: ManagedService,
    /// One of [`STATE_ACTIVE`], [`STATE_INACTIVE`], [`STATE_FAILED`],
    /// [`STATE_MISSING`], [`STATE_UNKNOWN`] — or whatever other word the
    /// beacon used, passed through verbatim rather than flattened into
    /// "unknown".
    pub state: String,
    /// The beacon's `reported_at`, so a confident-looking `active` from a
    /// five-day-old beacon is visibly five days old.
    pub reported_at: String,
    /// Why the state is what it is, when that is not self-evident.
    pub detail: String,
    /// Set when this unit's declared launchd domain is one its host cannot
    /// have. Carried on the row rather than recomputed by each surface,
    /// because the check needs the target's `role` and only the join has it.
    pub misdeclared_domain: Option<MisdeclaredDomain>,
}

impl ServiceStatus {
    pub fn to_json(&self) -> Value {
        let mut report = match self.service.to_json() {
            Value::Object(map) => map,
            other => return other,
        };
        report.insert("state".to_string(), json!(self.state));
        report.insert("reported_at".to_string(), json!(self.reported_at));
        report.insert("detail".to_string(), json!(self.detail));
        if let Some(misdeclared) = &self.misdeclared_domain {
            report.insert("misdeclared_domain".to_string(), misdeclared.to_json());
        }
        Value::Object(report)
    }
}

/// Resolve one unit's state out of a host beacon.
///
/// `beacon` is `None` when the host has published nothing at all, which is
/// a different fact from "the beacon does not carry this unit" and is kept
/// as a different state: conflating a silent host with a missing unit is
/// the class of mistake this whole module exists to stop.
fn beacon_state(beacon: Option<&Map<String, Value>>, unit_id: &str) -> (String, String) {
    let Some(beacon) = beacon else {
        return (
            STATE_UNKNOWN.to_string(),
            "host has published no health beacon".to_string(),
        );
    };
    let units = beacon.get("units").and_then(Value::as_object);
    let Some(entry) = units.and_then(|units| units.get(unit_id)) else {
        return (
            STATE_MISSING.to_string(),
            "declared here; the latest beacon does not report it".to_string(),
        );
    };
    // The beacon writer emits {"state": ...} per unit; older beacons wrote
    // a bare string. Both shapes are in flight, so read both.
    let state = match entry {
        Value::String(state) => state.clone(),
        Value::Object(fields) => fields
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    };
    if state.is_empty() {
        return (
            STATE_UNKNOWN.to_string(),
            "beacon reports the unit with no state".to_string(),
        );
    }
    (state, String::new())
}

/// A beacon older than the fleet's one silence threshold cannot describe the
/// present. Callers still receive its timestamp and the reason it was refused,
/// but never a confident `active` or `missing` derived from stale evidence.
fn stale_beacon_detail(reported_at: &str, now: DateTime<Utc>) -> Option<String> {
    let threshold = crate::monitor::host_silence::silence_threshold_seconds();
    let Some(stamp) = DateTime::parse_from_rfc3339(reported_at)
        .ok()
        .map(|stamp| stamp.with_timezone(&Utc))
    else {
        return Some("health beacon has no usable reported_at; unit state is unknown".to_string());
    };
    let age = now.signed_duration_since(stamp).num_seconds();
    if age < i64::default() || age <= threshold {
        return None;
    }
    Some(format!(
        "health beacon is {age}s old, past the {threshold}s silence threshold; unit state is unknown"
    ))
}

/// Every registry-managed service on every kind=local host, with the state
/// the latest beacons report.
///
/// Beacons only: no ssh, no per-host round trip, so this stays answerable
/// while a host is wedged. A host that has never published a beacon yields
/// [`STATE_UNKNOWN`] rows instead of an error, because one silent host must
/// not blank the fleet-wide answer.
pub async fn list_services(store: &JobStorage) -> Result<Vec<ServiceStatus>, DeployError> {
    let registry = super::host_channel::canonical_registry().await?;
    let mut rows: Vec<ServiceStatus> = Vec::new();
    for target in registry.local_targets() {
        let declared = declared_services(target);
        if declared.is_empty() {
            continue;
        }
        let report = match host_health::load_host_health(store, &target.name).await {
            Ok(report) => Some(report),
            Err(HostHealthError::NoBeacon { .. }) => None,
            Err(exc) => return Err(DeployError(exc.to_string())),
        };
        let beacon = report.as_ref().map(|report| &report.beacon);
        let reported_at = beacon
            .and_then(|beacon| beacon.get("reported_at"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let stale = report
            .as_ref()
            .and_then(|_| stale_beacon_detail(&reported_at, Utc::now()));
        for service in declared {
            let (mut state, mut detail) = beacon_state(beacon, service.unit_id());
            if let Some(stale) = &stale {
                state = STATE_UNKNOWN.to_string();
                detail = stale.clone();
            }
            let misdeclared_domain = MisdeclaredDomain::detect(target, &service);
            rows.push(ServiceStatus {
                service,
                state,
                reported_at: reported_at.clone(),
                detail,
                misdeclared_domain,
            });
        }
    }
    Ok(rows)
}

/// [`list_services`] narrowed to the units one NAME addresses. An empty
/// result is the caller's error to raise: "no managed service named X" and
/// "X is managed but reports nothing" are different answers.
pub async fn find_services(
    store: &JobStorage,
    name: &str,
) -> Result<Vec<ServiceStatus>, DeployError> {
    let mut rows = list_services(store).await?;
    rows.retain(|row| row.service.matches(name));
    Ok(rows)
}

// ---------------------------------------------------------------------------
// The approved channel
// ---------------------------------------------------------------------------

/// Everything the fixed remote programs report back through the
/// tab-delimited `STADO_*` markers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteReport {
    /// `uname -s` on the host.
    pub os: String,
    /// The launchd domain [`DOMAIN_RESOLVER`] chose for this unit (`system`,
    /// `gui/<uid>` or `user/<uid>`); empty on Linux, and empty on a Darwin
    /// host that has no per-login domain at all.
    pub domain: String,
    /// [`DOMAIN_STATUS_SYSTEM`], [`DOMAIN_STATUS_GRAPHICAL`],
    /// [`DOMAIN_STATUS_FALLBACK`] or [`DOMAIN_STATUS_UNAVAILABLE`] — how the
    /// resolver arrived at [`Self::domain`].
    pub domain_status: String,
    /// Why that domain, in the operator's words. Load-bearing for the
    /// fallback: it is the reason a user agent cannot be loaded, and until it
    /// was reported the fallback was a bare `user/501` note nobody read.
    pub domain_reason: String,
    /// The unit id the remote program actually addressed. On Linux this is
    /// the `.service` spelling, which differs from the launchd label.
    pub unit: String,
    /// The unit-file path the remote program actually resolved.
    pub path: String,
    /// The outcome word from the `STADO_SERVICE` marker.
    pub status: String,
    /// Flattened failure detail from the same marker.
    pub detail: String,
    /// `present` / `absent` from the adopt probe.
    pub file_state: String,
    /// `loaded` / `unloaded` from the adopt probe.
    pub unit_state: String,
    /// Remote exit status.
    pub exit_code: i32,
    /// Raw stdout, for the commands that carry a body after their marker.
    pub stdout: String,
    /// The end state this operation declared it would leave behind, empty
    /// for an operation that declares none.
    pub postcondition: String,
    /// What the host said about that end state:
    /// `host_channel::POSTCONDITION_MET` / `_UNMET` / `_UNOBSERVED`.
    pub postcondition_state: String,
    /// The probe's own words about what it found.
    pub postcondition_detail: String,
}

/// The unit is a system LaunchDaemon: launchd's `system` domain, loaded by
/// root.
pub const DOMAIN_STATUS_SYSTEM: &str = "system";
/// The unit is a LaunchAgent and its user has a graphical session, so the
/// domain is `gui/<uid>` — where a LaunchAgent actually lives.
pub const DOMAIN_STATUS_GRAPHICAL: &str = "graphical";
/// The unit is a LaunchAgent and nobody is logged in graphically, so the only
/// domain there is is the background `user/<uid>`. A user agent that needs
/// the login session cannot be loaded in it, which is why this word travels
/// with [`RemoteReport::domain_reason`] wherever it appears.
pub const DOMAIN_STATUS_FALLBACK: &str = "fallback";
/// launchd has no per-login domain for this login at all.
pub const DOMAIN_STATUS_UNAVAILABLE: &str = "unavailable";

/// The host ran the action and launchd has no job under the label in the
/// domain the action used.
///
/// A word of its own, and never one of the success words. `restarted` beside
/// `postcondition unmet` is the shape that hid this defect for weeks: a
/// report an operator reads top-down says the restart worked, and the unit is
/// not under launchd at all.
pub const STATUS_NOT_LOADED: &str = "not_loaded";

impl RemoteReport {
    /// The host's init system, from the OS it reported.
    pub fn kind(&self) -> &'static str {
        if self.os == "Darwin" {
            KIND_LAUNCHD
        } else {
            KIND_SYSTEMD
        }
    }

    /// True when the host was observed in the state the operation intended,
    /// or the operation declared no end state at all.
    pub fn postcondition_held(&self) -> bool {
        self.postcondition.is_empty() || self.postcondition_state == host_channel::POSTCONDITION_MET
    }

    /// True when the remote program reported the outcome the caller wanted
    /// AND the host was left in the state that outcome claims.
    ///
    /// Both halves, because the outage was exactly one half: the restart's
    /// own steps each did what they were written to do and the command
    /// reported on them faithfully, while the unit it was restarting ended
    /// up unloaded. A step that succeeds is not the same fact as a machine
    /// that works, and only the second one is worth calling success.
    pub fn succeeded(&self, expected: &str) -> bool {
        self.status == expected && self.postcondition_held()
    }

    /// A one-line failure message, preferring the marker detail over the
    /// bare status word.
    ///
    /// An unmet end state is printed BESIDE the operation's own outcome and
    /// never instead of it. `restarted; postcondition unmet: the unit is
    /// loaded and has a pid (no job at gui/501/com.wisent.weles-api)` is the
    /// sentence nobody had during the outage: either half alone sends an
    /// operator to the wrong place.
    pub fn failure(&self) -> String {
        let reported = if self.detail.is_empty() {
            self.status.clone()
        } else {
            format!("{}: {}", self.status, self.detail)
        };
        if self.postcondition_held() {
            return reported;
        }
        format!(
            "{reported}; postcondition {}: {} ({})",
            self.postcondition_state, self.postcondition, self.postcondition_detail
        )
    }

    /// True when the host ran the action and launchd has no job under the
    /// label in the domain that action used.
    pub fn unloaded(&self) -> bool {
        self.status == STATUS_NOT_LOADED
    }

    /// Turn a host-side [`STATUS_NOT_LOADED`] into the sentence the operator
    /// needs: the unit, the domain the action used, launchd's own words, and —
    /// when that domain is the per-login fallback — why a user agent cannot be
    /// loaded there. Composed here because the host's marker fields are cut to
    /// 160 characters and this has to say all of it.
    ///
    /// `action` is the verb in the operator's tense (`restart`, `deploy`), and
    /// it is named because the missing half of the old report was what the
    /// command thought it had done.
    fn name_unloaded(&mut self, unit: &str, action: &str) {
        if !self.unloaded() {
            return;
        }
        let mut detail = format!(
            "{unit} is not loaded in {}: {}. Nothing was started outside launchd, because a \
             process no unit owns dies with the login that spawned it and is not a {action}ed \
             service",
            self.domain, self.detail
        );
        if self.domain_status == DOMAIN_STATUS_FALLBACK {
            detail.push_str(&format!(". {}", self.domain_reason));
        }
        self.detail = detail;
    }

    pub fn to_json(&self) -> Value {
        let mut report = json!({
            "os": self.os,
            "unit": self.unit,
            "path": self.path,
            "status": self.status,
            "detail": self.detail,
            "exit_code": self.exit_code,
        });
        // One object, the same one `host recover` prints, wherever a domain is
        // named at all: the name alone was what an operator had to act on, and
        // `user/501` alone does not say that it is a fallback or what the
        // fallback costs.
        if !(self.domain.is_empty() && self.domain_status.is_empty()) {
            report["launchd_domain"] = json!({
                "name": self.domain,
                "status": self.domain_status,
                "reason": self.domain_reason,
            });
        }
        if !self.postcondition.is_empty() {
            report["postcondition"] = json!({
                "intent": self.postcondition,
                "state": self.postcondition_state,
                "detail": self.postcondition_detail,
            });
        }
        report
    }
}

/// Feed one fixed remote program to one host over the shared channel.
async fn run_remote(
    target: &ComputeTarget,
    script: String,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report_from(output))
}

/// Feed one fixed remote program to one host and check, on the host and
/// before the connection closes, that it left behind the state it intended.
///
/// The prelude travels separately from the body because the probe is armed
/// between them: `host_channel::PostCondition::arm` explains why the check
/// cannot simply be appended to a body whose success path is an early
/// `exit`.
async fn run_remote_checked(
    target: &ComputeTarget,
    prelude: &str,
    body: &str,
    postcondition: &host_channel::PostCondition,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let (output, verdict) =
        host_channel::run_checked_script(target, prelude, body, postcondition, runner).await?;
    let mut report = report_from(output);
    report.postcondition = verdict.describe;
    report.postcondition_state = verdict.state;
    report.postcondition_detail = verdict.detail;
    Ok(report)
}

/// One transport result as a report, whether or not the operation declared
/// an end state.
fn report_from(output: CommandOutput) -> RemoteReport {
    let mut report = parse_markers(&output.stdout);
    report.exit_code = output.code;
    if report.status.is_empty() && !output.ok() {
        // ssh itself failed (unreachable host, refused key), so there are
        // no markers to read: surface the transport's own last word, the
        // way every other command on this channel does.
        report.status = host_channel::FAILED_STATUS.to_string();
        report.detail = host_channel::last_error_line(&output, "ssh failed");
    }
    report.stdout = output.stdout;
    report
}

/// Fold the `STADO_*` marker lines of stdout into a [`RemoteReport`].
///
/// Same protocol and framing as
/// `deploy/host_recovery.rs::parse_output`; matched with slice patterns so
/// a marker with the wrong arity falls through instead of being mis-read.
pub fn parse_markers(stdout: &str) -> RemoteReport {
    let mut report = RemoteReport::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_HOST", os, domain, unit, path] => {
                report.os = (*os).to_string();
                report.domain = (*domain).to_string();
                report.unit = (*unit).to_string();
                report.path = (*path).to_string();
            }
            ["STADO_SERVICE", _unit, status, detail] => {
                report.status = (*status).to_string();
                report.detail = (*detail).to_string();
            }
            ["STADO_DOMAIN", domain, status, reason] => {
                report.domain = (*domain).to_string();
                report.domain_status = (*status).to_string();
                report.domain_reason = (*reason).to_string();
            }
            ["STADO_ADOPT", file_state, unit_state] => {
                report.file_state = (*file_state).to_string();
                report.unit_state = (*unit_state).to_string();
            }
            _ => {}
        }
    }
    report
}

/// Split `stdout` at the first `marker` line, returning that line's single
/// trailing field and everything after it. The commands that carry a body
/// (a log tail, a unit file) announce it with a marker and then stream it
/// raw, so the body needs no framing of its own.
pub fn split_marker_body<'a>(stdout: &'a str, marker: &str) -> Option<(&'a str, &'a str)> {
    let mut rest = stdout;
    loop {
        let (line, tail) = rest.split_once('\n').unwrap_or((rest, ""));
        if let Some(field) = line
            .strip_prefix(marker)
            .and_then(|head| head.strip_prefix('\t'))
        {
            return Some((field, tail));
        }
        if tail.is_empty() {
            return None;
        }
        rest = tail;
    }
}

/// Split a log tail at the `STADO_ERR` section the logs program emits after
/// the stdout tail on Darwin. The marker line's field is the stderr file,
/// or the reason there is nothing to show ("absent in plist", "<path>
/// (empty)"); everything after the marker line is the stderr tail. Linux
/// tails carry no such marker — the journal merges the streams — and pass
/// through whole.
fn split_error_section(tail: &str) -> (&str, Option<(&str, &str)>) {
    let Some(index) = tail.find("\nSTADO_ERR\t") else {
        return (tail, None);
    };
    let section = &tail[index + 1..];
    let (line, error_body) = section.split_once('\n').unwrap_or((section, ""));
    let error_origin = line.strip_prefix("STADO_ERR\t").unwrap_or_default();
    (&tail[..index + 1], Some((error_origin, error_body)))
}

// ---------------------------------------------------------------------------
// Splicing operator data into the fixed remote programs
// ---------------------------------------------------------------------------

/// Splice a unit-file path into a fixed remote program.
///
/// Registry-declared paths use the `$HOME/...` idiom —
/// `host_recovery::MANAGED_AGENTS` spells every plist that way, and the
/// recovery script splices them inside double quotes for exactly this
/// reason — so `shlex_quote` is wrong here: it would ship a literal `$HOME`
/// and every lookup would miss. Double quotes keep the expansion, and are
/// only safe on a vetted charset, so anything that could open a command
/// substitution, escape the quotes or add a line is refused outright rather
/// than escaped into something subtle. An empty path means "let the remote
/// program derive it".
pub fn quote_unit_path(path: &str) -> Result<String, DeployError> {
    if path.is_empty() {
        return Ok(String::new());
    }
    let body = path.strip_prefix(HOME_PREFIX).unwrap_or(path);
    let safe = |ch: char| ch.is_ascii_alphanumeric() || "_-./+@:".contains(ch);
    if body.chars().all(safe) {
        return Ok(path.to_string());
    }
    Err(DeployError(format!(
        "unit path {} contains characters that cannot ride the fixed remote program",
        py_str_repr(path)
    )))
}

/// Validate one remote destination file independently of shell quoting.
///
/// The commands that write a file on a managed host deliberately support only
/// an absolute path or a path rooted at the target user's home. The value
/// travels base64-encoded, but rejecting parent traversal keeps a typo from
/// turning a credential sync into an unrelated file rewrite. `label` names the
/// destination the way the operator asked for it -- an environment file for
/// `service secret-sync`, a token file for `service token-file-sync` -- so a
/// refusal says which of a command's paths was wrong.
fn validate_home_rooted_file(path: &str, label: &str) -> Result<(), DeployError> {
    let local = path.strip_prefix("$HOME/").unwrap_or(path);
    let rooted = path.starts_with('/') || path.starts_with("$HOME/");
    let usable_file = Path::new(local).file_name().is_some();
    let traverses_parent = Path::new(local)
        .components()
        .any(|part| matches!(part, Component::ParentDir));
    if rooted && usable_file && !traverses_parent && !path.chars().any(char::is_control) {
        return Ok(());
    }
    Err(DeployError(format!(
        "{label} {} must be an absolute or home-relative file path without parent traversal",
        py_str_repr(path)
    )))
}

fn validate_env_variable(variable: &str) -> Result<(), DeployError> {
    let mut chars = variable.chars();
    let head_ok = chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_uppercase());
    let tail_ok = chars.all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit());
    if head_ok && tail_ok {
        return Ok(());
    }
    Err(DeployError(format!(
        "environment variable {} must match [A-Z_][A-Z0-9_]*",
        py_str_repr(variable)
    )))
}

fn validate_secret_value(value: &str) -> Result<(), DeployError> {
    if !value.is_empty() && !value.chars().any(char::is_control) {
        return Ok(());
    }
    Err(DeployError(
        "secret value must be non-empty and single-line".to_string(),
    ))
}

fn validate_loopback_probe_url(raw: &str) -> Result<(), DeployError> {
    let parsed = url::Url::parse(raw)
        .map_err(|error| DeployError(format!("invalid service probe URL: {error}")))?;
    let loopback = parsed
        .host_str()
        .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "::1"));
    if parsed.scheme() == "http"
        && loopback
        && parsed.port().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none()
    {
        return Ok(());
    }
    Err(DeployError(format!(
        "service probe URL {} must be an explicit loopback HTTP endpoint without credentials or a fragment",
        py_str_repr(raw)
    )))
}

/// A body line equal to the heredoc delimiter would end the heredoc early
/// and hand the rest of the unit to the shell as commands. Nothing this
/// crate renders contains such a line; refuse rather than assume.
fn guard_heredoc(content: &str) -> Result<(), DeployError> {
    if content.lines().any(|line| line.trim() == UNIT_HEREDOC) {
        return Err(DeployError(format!(
            "rendered unit contains the reserved delimiter line {}",
            py_str_repr(UNIT_HEREDOC)
        )));
    }
    Ok(())
}

/// The registry's own target-name rule, applied to a service name because
/// the name becomes part of a launchd label, part of a systemd unit name,
/// and a field of the canonical document. Mirrors the check
/// `targets.rs::validate_registry` runs on `registry.targets[].name`.
pub(crate) fn validate_service_name(name: &str) -> Result<(), DeployError> {
    let inner = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ".-_".contains(ch);
    let edge = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit();
    let head_ok = name.chars().next().is_some_and(edge);
    let tail_ok = name.chars().next_back().is_some_and(edge);
    if head_ok && tail_ok && name.chars().all(inner) {
        return Ok(());
    }
    Err(DeployError(format!(
        "service name {} must be a lowercase identifier of letters, digits, '.', '-' and '_'",
        py_str_repr(name)
    )))
}

/// The program a deployed unit runs. It is interpolated raw into the plist
/// (`local_install::plist_text` does no XML escaping, matching the Python
/// it was ported from) and into the systemd `ExecStart`, so it has to be
/// well-formed for both without escaping.
fn validate_program(program: &str) -> Result<(), DeployError> {
    if !program.starts_with('/') {
        return Err(DeployError(format!(
            "--from {} must be an absolute path on the target host",
            py_str_repr(program)
        )));
    }
    if program
        .chars()
        .any(|ch| ch.is_control() || "<>&\"'".contains(ch))
    {
        return Err(DeployError(format!(
            "--from {} contains characters that cannot be rendered into a unit file",
            py_str_repr(program)
        )));
    }
    Ok(())
}

/// An argument the deployed unit is started with. It lands in the same two
/// places as the program and under the same no-escaping rule, and a unit
/// whose arguments are empty strings is a unit nobody can read back from
/// `service show`, so both are refused here rather than at the host.
fn validate_unit_argument(arg: &str) -> Result<(), DeployError> {
    if arg.is_empty() {
        return Err(DeployError(
            "--arg cannot be empty; drop it instead".to_string(),
        ));
    }
    if arg
        .chars()
        .any(|ch| ch.is_control() || "<>&\"'".contains(ch))
    {
        return Err(DeployError(format!(
            "--arg {} contains characters that cannot be rendered into a unit file",
            py_str_repr(arg)
        )));
    }
    Ok(())
}

/// Reject a unit id that cannot ride the remote program as a shell word.
/// `shlex_quote` handles the quoting, but a control character in a launchd
/// label is never a real unit and would corrupt the marker framing.
pub fn validate_unit_id(unit: &str) -> Result<(), DeployError> {
    if unit.is_empty() || unit.chars().any(char::is_control) {
        return Err(DeployError(format!(
            "unit {} is not a usable launchd label or systemd unit name",
            py_str_repr(unit)
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The fixed remote programs
// ---------------------------------------------------------------------------

/// Shared head of every remote program: identify the OS, resolve the
/// launchd domain the way the recovery script does, derive the unit path
/// when the caller did not declare one, and define the marker emitter.
///
/// `say` flattens its detail exactly like `host_recovery`'s script (`tr`
/// over tab/CR/LF, then `cut`) so one marker can never span two lines and
/// desynchronise the parser. Written as an escaped string, not a raw
/// string, for the same reason the recovery script is.
const REMOTE_PRELUDE: &str = "set -u
unit=@UNIT@
linux_unit=@LINUX_UNIT@
unit_path=\"@PATH@\"
os=$(/usr/bin/uname -s)
uid=$(/usr/bin/id -u)
gui=\"gui/$uid\"
user_domain=\"user/$uid\"
domain=\"\"
domain_status=\"\"
domain_reason=\"\"
launch=/bin/launchctl
say() {
  detail=$(printf '%s' \"$2\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-400)
  printf 'STADO_SERVICE\\t%s\\t%s\\t%s\\n' \"$unit\" \"$1\" \"$detail\"
}
@DOMAIN_RESOLVER@@UNIT_STATE@if [ \"$os\" = \"Darwin\" ]; then
  # The file first, the domain second. An unqualified label may name this
  # login's agent or a system daemon, and which domain the unit belongs to
  # follows from the file -- so resolving a domain before knowing which file
  # this is, and patching it afterwards, is how one command came to act in one
  # domain, probe another, and report a third. The search covers
  # /Library/LaunchDaemons as well as this login's LaunchAgents because
  # adoption used to look only in the second and reported a running always-on
  # daemon as absent.
  if [ -z \"$unit_path\" ]; then
    if [ -f \"$HOME/Library/LaunchAgents/$unit.plist\" ]; then
      unit_path=\"$HOME/Library/LaunchAgents/$unit.plist\"
    elif [ -f \"/Library/LaunchDaemons/$unit.plist\" ]; then
      unit_path=\"/Library/LaunchDaemons/$unit.plist\"
    else
      unit_path=\"$HOME/Library/LaunchAgents/$unit.plist\"
    fi
  fi
  if ! stado_domain_of \"$unit_path\"; then
@NO_DOMAIN@
  fi
@OBSERVED_DOMAIN@
elif [ \"$os\" = \"Linux\" ]; then
  # The same search the Darwin branch above makes, for the same reason it was
  # widened: adoption looked only at this login's user units and reported a
  # running system unit as absent. On 2026-09-03 that unit was
  # `wisent-compute-agent.service` on the fleet's only linux-amd64 builder --
  # loaded, running a stado image that refuses today's registry document, and
  # so unmanaged that nothing could cycle it while every linux release build
  # queued behind it.
  if [ -n \"$linux_unit\" ]; then unit=\"$linux_unit\"; fi
  if [ -z \"$unit_path\" ]; then
    if [ -f \"$HOME/.config/systemd/user/$unit\" ]; then
      unit_path=\"$HOME/.config/systemd/user/$unit\"
    elif [ -f \"/etc/systemd/system/$unit\" ]; then
      unit_path=\"/etc/systemd/system/$unit\"
    else
      unit_path=\"$HOME/.config/systemd/user/$unit\"
    fi
  fi
  case \"$unit_path\" in
    /etc/systemd/system/*) scope=system ;;
    *) scope=user ;;
  esac
  domain=\"$scope\"
  if [ \"$scope\" = \"system\" ]; then
    systemd_detail='systemd system scope'
  else
    systemd_detail='systemd --user'
  fi
  case \"$unit_path\" in
    */.config/systemd/user/*) owner_path=\"${unit_path%%/.config/systemd/user/*}\" ;;
    *) owner_path=\"$unit_path\" ;;
  esac
  while [ ! -e \"$owner_path\" ] && [ \"$owner_path\" != \"/\" ]; do
    owner_path=$(/usr/bin/dirname \"$owner_path\")
  done
  service_user=$(/usr/bin/stat -c %U \"$owner_path\")
  service_uid=$(/usr/bin/id -u \"$service_user\")
  if [ -x /usr/bin/sudo ]; then sudo_bin=/usr/bin/sudo; else sudo_bin=/bin/sudo; fi
  stado_root() {
    if [ \"$uid\" = \"0\" ]; then
      \"$@\"
      return
    fi
    \"$sudo_bin\" -n \"$@\"
  }
  stado_systemctl() {
    # A system unit is root's job and has no per-user bus: addressing it with
    # `--user` is what made every verb in this module answer \"not present\"
    # for a unit the host was plainly running.
    if [ \"$scope\" = \"system\" ]; then
      stado_root /usr/bin/systemctl \"$@\"
      return
    fi
    runtime=\"/run/user/$service_uid\"
    if [ \"$service_uid\" = \"$uid\" ]; then
      /usr/bin/env \
        XDG_RUNTIME_DIR=\"$runtime\" \
        DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
        /usr/bin/systemctl --user \"$@\"
      return
    fi
    \"$sudo_bin\" -n -u \"$service_user\" /usr/bin/env \
      XDG_RUNTIME_DIR=\"$runtime\" \
      DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
      /usr/bin/systemctl --user \"$@\"
  }
else
  say 'unsupported_os' \"$os\"
  exit 65
fi
printf 'STADO_HOST\\t%s\\t%s\\t%s\\t%s\\n' \"$os\" \"$domain\" \"$unit\" \"$unit_path\"
if [ \"$os\" = \"Darwin\" ]; then
  printf 'STADO_DOMAIN\\t%s\\t%s\\t%s\\n' \"$domain\" \"$domain_status\" \"$(printf '%s' \"$domain_reason\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-400)\"
fi
";

/// The one answer to "which launchd domain does this unit belong to", read by
/// every program in this module and by `host_recovery`'s recovery pass.
///
/// Three domains, and the difference between the last two is the defect this
/// function exists for:
///
/// - `/Library/LaunchDaemons/...` is root's job: the `system` domain, reached
///   with sudo.
/// - A LaunchAgent of a user who has a graphical session lives in
///   `gui/<uid>`, and an ssh login can address that domain while the session
///   exists.
/// - A LaunchAgent of a user who has none has only the background per-user
///   domain `user/<uid>` — the domain an ssh login is itself placed in, and
///   the one an agent that needs the login session cannot be loaded into.
///
/// The graphical session is read the way macOS exposes it, and the check was
/// chosen against the live host rather than guessed: `/dev/console` is owned
/// by the user holding the graphical session and by root at the login window,
/// and launchd has a `gui/<uid>` domain only while that session exists. Both
/// halves are required, so the reported domain is one the next `launchctl`
/// verb can actually address.
///
/// What that read answers on control-host on 2026-08-19, through
/// `stado host exec` (read-only, allowlisted): `who` prints nothing,
/// `loginwindow` runs as root, no `Dock`, `Finder` or `SystemUIServer`
/// process exists for any account, and the login's own `launchctl list`
/// holds 62 background `com.apple.*` agents and no `com.wisent.*` label.
/// Nobody is logged in graphically there, so `gui/501` does not exist, and
/// the honest answer for that host's agent is the `user/501` fallback —
/// reported as the reason the agent cannot be loaded instead of papered over
/// with a bare process.
///
/// Sets `$domain` (what every verb addresses and every probe reads),
/// `$domain_status` ([`DOMAIN_STATUS_SYSTEM`], [`DOMAIN_STATUS_GRAPHICAL`],
/// [`DOMAIN_STATUS_FALLBACK`] or [`DOMAIN_STATUS_UNAVAILABLE`]),
/// `$domain_reason` (the operator's sentence for that choice) and `$launch`
/// (the launchctl this domain needs). Returns non-zero only when launchd has
/// no per-login domain at all, which is the one case a caller may answer
/// differently.
pub const DOMAIN_RESOLVER: &str = "stado_domain_of() {
  domain=\"\"
  domain_status=\"\"
  domain_reason=\"\"
  launch=/bin/launchctl
  case \"$1\" in
    /Library/LaunchDaemons/*)
      domain='system'
      domain_status='system'
      domain_reason='a unit in /Library/LaunchDaemons is a system LaunchDaemon, so its job belongs to the system domain and loading it needs root'
      launch=\"/usr/bin/sudo -n /bin/launchctl\"
      return 0
      ;;
  esac
  account=$(/usr/bin/id -un)
  console=$(/usr/bin/stat -f%Su /dev/console 2>/dev/null | /usr/bin/tr -d ' \t\r\n')
  if [ -z \"$console\" ]; then console='nobody'; fi
  if [ \"$console\" = \"$account\" ] && /bin/launchctl print \"$gui\" >/dev/null 2>&1; then
    domain=\"$gui\"
    domain_status='graphical'
    domain_reason=\"$account owns /dev/console and launchd has $gui, so a LaunchAgent of this login loads there\"
    return 0
  fi
  if /bin/launchctl print \"$user_domain\" >/dev/null 2>&1; then
    domain=\"$user_domain\"
    domain_status='fallback'
    domain_reason=\"/dev/console belongs to $console, not $account: no graphical session, so $gui does not exist and a LaunchAgent has only the background domain $user_domain\"
    return 0
  fi
  domain_status='unavailable'
  domain_reason=\"launchd has neither $gui nor $user_domain for $account\"
  return 1
}
";

// ---------------------------------------------------------------------------
// The session behind the domain, asked as a question
// ---------------------------------------------------------------------------

/// Marker word the session probe frames its one answer in, in the same
/// tab-delimited `STADO_*` family every other program on this channel speaks.
const SESSION_MARKER: &str = "STADO_SESSION";

/// `session.kind` when an account holds the screen: [`DOMAIN_RESOLVER`] found
/// the console owned by this login and launchd holding that login's
/// `gui/<uid>`.
pub const SESSION_GRAPHICAL: &str = "graphical";
/// `session.kind` when nobody holds the screen.
///
/// This one word is the whole of control-host's condition, and the start
/// of the chain nothing in this product could previously state: no graphical
/// session, so launchd builds no `gui/<uid>`, so a LaunchAgent has nowhere to
/// load, so the host publishes no capacity, so a job pinned to it waits.
pub const SESSION_HEADLESS: &str = "headless";
/// `session.kind` when the probe could not answer — the host did not answer
/// at all, the read failed, or it is not a macOS host. Never a guess in
/// either direction: a diagnostic that could not read a fact says so, and an
/// unreadable session must not make a readable host look unreadable.
pub const SESSION_UNKNOWN: &str = "unknown";

/// The wall-clock cap on the session read.
///
/// Deliberately far under the channel's own
/// [`host_channel::remote_timeout`]: this is four `exec`s behind an ssh hop
/// the shared options already bound at `ConnectTimeout=15`, so thirty seconds
/// leaves the reads fifteen of their own. A probe that has not answered by
/// then is [`SESSION_UNKNOWN`] — a diagnostic that hangs on one of its facts
/// is worse than one that reports that fact as unread.
pub const SESSION_TIMEOUT_SECONDS: u64 = 30;

/// The read-only half of [`DOMAIN_RESOLVER`]: who owns the console, whether
/// launchd has a graphical domain, and nothing else.
///
/// `stado service restart` has performed exactly this determination since the
/// fallback domain was named, and until now it took a restart — a write, on a
/// host that is already the wrong shape — to see the answer. This is that
/// same function asked as a question. It resolves a per-login path so the
/// system branch cannot short-circuit the two checks it exists to make, and
/// it bootstraps nothing, kickstarts nothing and loads nothing.
///
/// The reason is flattened over tab/CR/LF, because a marker that spans two
/// lines desynchronises the parser — but it is deliberately NOT cut at 160
/// characters the way the `STADO_DOMAIN` marker cuts it. The reason IS the
/// answer here rather than a note beside one, and a truncated sentence is not
/// the resolver's sentence.
const SESSION_PROBE: &str = "set -u
os=$(/usr/bin/uname -s)
if [ \"$os\" != Darwin ]; then
  printf 'STADO_SESSION\\t%s\\t%s\\t%s\\n' 'unsupported' '' \"$os has no console session of the kind a per-login unit needs\"
  exit 0
fi
uid=$(/usr/bin/id -u)
gui=\"gui/$uid\"
user_domain=\"user/$uid\"
console=''
@DOMAIN_RESOLVER@stado_domain_of \"$HOME/Library/LaunchAgents\"
printf 'STADO_SESSION\\t%s\\t%s\\t%s\\n' \"$domain_status\" \"$console\" \"$(printf '%s' \"$domain_reason\" | /usr/bin/tr '\t\r\n' ' ')\"
";

/// What one host answered about whether anybody is logged in on its screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSession {
    /// [`SESSION_GRAPHICAL`], [`SESSION_HEADLESS`] or [`SESSION_UNKNOWN`].
    pub kind: &'static str,
    /// Who owns `/dev/console`: the login name while a graphical session
    /// exists, `root` at the login window. `None` when the probe could not
    /// answer, never an invented name.
    pub console_owner: Option<String>,
    /// [`DOMAIN_RESOLVER`]'s own sentence for this answer, verbatim — the
    /// same words `stado service restart --json` prints under
    /// `launchd_domain.reason`. When the probe could not answer, this is why.
    pub detail: String,
}

impl HostSession {
    /// The answer for a probe that did not produce one, carrying why.
    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            kind: SESSION_UNKNOWN,
            console_owner: None,
            detail: detail.into(),
        }
    }

    /// True only for [`SESSION_HEADLESS`]. [`SESSION_UNKNOWN`] is not
    /// headless: acting on a fact nobody read is how a diagnostic starts
    /// inventing findings.
    pub fn is_headless(&self) -> bool {
        self.kind == SESSION_HEADLESS
    }

    /// The probe's one marker line, read out of its stdout.
    pub fn parse(stdout: &str) -> Self {
        let Some(fields) = stdout
            .lines()
            .map(host_channel::marker_fields)
            .find(|fields| fields.first() == Some(&SESSION_MARKER))
        else {
            return Self::unknown(
                "this host ran the session read and printed no answer to it".to_string(),
            );
        };
        let [_, status, console, reason] = fields[..] else {
            return Self::unknown(format!(
                "this host's session answer came back in {} field(s) instead of 4",
                fields.len()
            ));
        };
        // `unavailable` is headless too, and not a failed read: launchd having
        // no `gui/<uid>` for this login is precisely the absence of a
        // graphical session, whatever the console says about who owns it.
        let kind = match status {
            DOMAIN_STATUS_GRAPHICAL => SESSION_GRAPHICAL,
            DOMAIN_STATUS_FALLBACK | DOMAIN_STATUS_UNAVAILABLE => SESSION_HEADLESS,
            _ => SESSION_UNKNOWN,
        };
        Self {
            kind,
            console_owner: (!console.is_empty()).then(|| console.to_string()),
            detail: reason.to_string(),
        }
    }

    /// The one line an operator reads first, in the words they use for it.
    ///
    /// No `gui/<uid>`, no domain and no bootstrap here. Those are true, they
    /// are what the next command needs, and they belong in [`Self::detail`]
    /// underneath — not in the sentence that answers "is anyone logged in".
    pub fn headline(&self) -> String {
        match (self.kind, self.console_owner.as_deref()) {
            (SESSION_GRAPHICAL, Some(owner)) => {
                format!("{owner} is logged in on the screen here")
            }
            (SESSION_GRAPHICAL, None) => "someone is logged in on the screen here".to_string(),
            (SESSION_HEADLESS, _) => "nobody is logged in on the screen here".to_string(),
            _ => "whether anyone is logged in on the screen here could not be read".to_string(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "console_owner": self.console_owner,
            "detail": self.detail,
        })
    }
}

/// Ask one host whether anybody is logged in on its screen.
///
/// Read-only and infallible by construction: every way this can fail — an
/// unresolvable key, a refused connection, a remote non-zero, a missing
/// marker — comes back as [`SESSION_UNKNOWN`] carrying the failure's own
/// words. A caller diagnosing a sick host must never lose the facts it
/// already has because one more optional read did not land.
pub async fn read_session(target: &ComputeTarget, runner: &Runner) -> HostSession {
    let probe = SESSION_PROBE.replace("@DOMAIN_RESOLVER@", DOMAIN_RESOLVER);
    match host_channel::run_script_with_timeout(
        target,
        &probe,
        std::time::Duration::from_secs(SESSION_TIMEOUT_SECONDS),
        runner,
    )
    .await
    {
        Ok(output) if output.ok() => HostSession::parse(&output.stdout),
        Ok(output) => HostSession::unknown(host_channel::last_error_line(
            &output,
            "this host refused the session read and said nothing about why",
        )),
        Err(exc) => HostSession::unknown(exc.to_string()),
    }
}

/// The three reads every body and every probe makes about one unit, in one
/// place: what the unit declares it runs, which processes are running exactly
/// that, and what launchd itself says about the label.
///
/// `stado_unit_pids` is the one that had to change. It used to be
/// `pgrep -f "^$program"` against the unit's program path, and on a host
/// where every Stado service runs one binary that pattern is every service:
/// on 2026-08-19 a unit-scoped `stado service restart
/// com.wisent.always-on.stado-object-api --host control-host` ended
/// eight processes — the object API, the host's resolver holding
/// 17600/17601/17612/17621, and a bare agent — because every one of them runs
/// `/Users/charles/.stado/bin/stado`, and it reported `restarted` with a met
/// postcondition afterwards. `KeepAlive` brought them back; one non-KeepAlive
/// sibling would have stayed down. The distinguishing fact is the argv the
/// unit declares (`dashboard --bind 127.0.0.1 --port 8765` against `resolver
/// serve --target <host>`), so the whole argv is matched, and where launchd
/// will answer for the label at all its own pid is preferred to any pattern.
const UNIT_STATE: &str = "stado_unit_argv() {
  if [ ! -f \"$1\" ]; then return 0; fi
  argv_read=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' \"$1\" 2>/dev/null | /usr/bin/awk 'NR == 1 && $0 == \"Array {\" { next } $0 == \"}\" { next } { sub(/^[[:space:]]+/, \"\"); sub(/[[:space:]]+$/, \"\"); printf \"%s%s\", separator, $0; separator = \" \" }')
  if [ -z \"$argv_read\" ]; then
    argv_read=$(/usr/libexec/PlistBuddy -c 'Print :Program' \"$1\" 2>/dev/null)
    # PlistBuddy answers a missing key on stdout, so a program is accepted
    # only in the one shape a program has: an absolute path.
    case \"$argv_read\" in /*) ;; *) argv_read='' ;; esac
  fi
  printf '%s' \"$argv_read\"
}
stado_unit_pids() {
  pids_want=\"$1\"
  pids_program=\"${pids_want%% *}\"
  if [ -z \"$pids_program\" ]; then return 0; fi
  pids_args=''
  case \"$pids_want\" in *' '*) pids_args=\" ${pids_want#* }\" ;; esac
  # A unit that runs .../services/NAME/current/... names a link, and a process
  # that outlived a relink shows the version directory that link resolved to.
  # Both spellings share the service directory, so that is what the candidate
  # scan is widened to; the argv below is what decides.
  pids_root=\"$pids_program\"
  case \"$pids_program\" in */current/*) pids_root=\"${pids_program%%/current/*}/\" ;; esac
  pids_found=''
  for pids_pid in $(/usr/bin/pgrep -f \"^$pids_root\" 2>/dev/null); do
    pids_have=$(/bin/ps -p \"$pids_pid\" -o command= 2>/dev/null | /usr/bin/tr -s ' ' | /usr/bin/sed 's/^ //;s/ $//')
    if [ -z \"$pids_have\" ]; then continue; fi
    if [ \"$pids_have\" = \"$pids_want\" ]; then
      pids_found=\"$pids_found$pids_pid \"
      continue
    fi
    pids_have_args=''
    case \"$pids_have\" in *' '*) pids_have_args=\" ${pids_have#* }\" ;; esac
    if [ \"$pids_have_args\" != \"$pids_args\" ]; then continue; fi
    case \"${pids_have%% *}\" in \"$pids_root\"*) pids_found=\"$pids_found$pids_pid \" ;; esac
  done
  printf '%s' \"${pids_found% }\"
}
stado_launchd_state() {
  pc_pid=''
  if pc_info=$($launch print \"$domain/$unit\" 2>&1); then
    pc_loaded=yes
    pc_pid=$(printf '%s\\n' \"$pc_info\" | /usr/bin/awk '$1 == \"pid\" && $2 == \"=\" { print $3; exit }')
  else
    pc_loaded=no
  fi
}
";

/// What the prelude does on a Darwin host whose per-login launchd domain does
/// not exist at all, for every command that addresses an installed unit.
///
/// A restart, a stop or a retire aimed at a domain that is not there has
/// nothing to act on, and inventing one would mean installing a unit in the
/// middle of an operation that promised only to touch an existing one.
const NO_DOMAIN_REFUSE: &str = "    say 'no_launchd_domain' \"$domain_reason\"
    exit 66";

/// What [`ensure_service`] does instead: install into the system domain.
///
/// `launchctl bootstrap gui/$uid` over ssh answers `Could not switch to audit
/// session ... Operation not permitted`, and `stado service deploy` returned
/// that failure having installed nothing — which is how two `stado agent`
/// processes came to run for four days with no unit behind them. The system
/// domain is the one that does exist on an ssh login, so the unit that gets
/// installed is the daemon spelling of the same job, in
/// `/Library/LaunchDaemons`, and [`DOMAIN_RESOLVER`] then resolves every
/// later command to `system` from that path alone.
const NO_DOMAIN_SYSTEM: &str = "    domain=\"system\"
    domain_status='system'
    domain_reason='launchd has no per-login domain on this login, so the job is installed as a system LaunchDaemon instead'
    launch=\"/usr/bin/sudo -n /bin/launchctl\"
    unit_path=\"/Library/LaunchDaemons/$unit.plist\"";

// ---------------------------------------------------------------------------
// The end states the lifecycle operations intend
// ---------------------------------------------------------------------------

/// The end state a restart or a start intends.
///
/// Both halves are load-bearing. A unit can be loaded with nothing running
/// under it (launchd accepted the job and the program died on start), and a
/// program can be running with no unit loaded — that second one is what the
/// last-resort fallbacks in these scripts used to produce, and reporting it
/// as a successful restart is how an operator comes to believe a service is
/// under management when the next logout will end it.
const RUNNING_DESCRIBE: &str = "the unit is loaded and has a pid";

/// Read in the domain the action used, because that is the only domain whose
/// answer means anything: `no job at user/501/<label>` is a failure when the
/// action bootstrapped into `user/501` and says nothing at all about a job
/// the action never addressed.
const RUNNING_PROBE: &str = "  if [ \"$os\" = \"Darwin\" ]; then
    stado_launchd_state
    if [ \"$pc_loaded\" = no ]; then
      stado_post 'unmet' \"no job at $domain/$unit\"
    elif [ -n \"$pc_pid\" ]; then
      stado_post 'met' \"$domain/$unit pid $pc_pid\"
    else
      stado_post 'unmet' \"$domain/$unit is loaded with no pid\"
    fi
  elif stado_systemctl is-active --quiet \"$unit\"; then
    pc_pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
    if [ -n \"$pc_pid\" ] && [ \"$pc_pid\" != 0 ]; then
      stado_post 'met' \"$unit pid $pc_pid\"
    else
      stado_post 'unmet' \"$unit is active with no main pid\"
    fi
  else
    stado_post 'unmet' \"$unit is not active\"
  fi
";

/// The end state a stop intends.
///
/// A booted-out label is not the same fact as a stopped service: the sweep
/// these bodies run exists because a program started once outside its own
/// label survives every `bootout`, keeps the listening socket, and makes
/// every later start die on `address already in use`. So the probe asks
/// whether anything is running under the unit, not whether the label is
/// gone; a loaded job with no pid is a stopped service and says so.
const STOPPED_DESCRIBE: &str = "the unit is not running";

const STOPPED_PROBE: &str = "  if [ \"$os\" = \"Darwin\" ]; then
    # launchctl bootout returns before an exiting job disappears from
    # `launchctl print`. Wait for that declared end state instead of reporting
    # a failed stop that becomes true moments after the command returns.
    stopped_attempt=0
    while [ \"$stopped_attempt\" -lt 30 ]; do
      stado_launchd_state
      if [ \"$pc_loaded\" = no ] || [ -z \"$pc_pid\" ]; then break; fi
      stopped_attempt=$((stopped_attempt + 1))
      /bin/sleep 1
    done
    if [ \"$pc_loaded\" = no ]; then
      stado_post 'met' \"no job at $domain/$unit\"
    elif [ -n \"$pc_pid\" ]; then
      stado_post 'unmet' \"$domain/$unit still running as pid $pc_pid\"
    else
      stado_post 'met' \"$domain/$unit is loaded but not running\"
    fi
  else
    stopped_attempt=0
    while [ \"$stopped_attempt\" -lt 30 ]; do
      if ! stado_systemctl is-active --quiet \"$unit\"; then break; fi
      stopped_attempt=$((stopped_attempt + 1))
      /bin/sleep 1
    done
    if stado_systemctl is-active --quiet \"$unit\"; then
      stado_post 'unmet' \"$unit is still active\"
    else
      stado_post 'met' \"$unit is not active\"
    fi
  fi
";

/// One declared end state. The probe reads the host through the same prelude
/// vocabulary the body does — `$domain` above all — so the check cannot end
/// up asking about a domain the operation never acted in.
fn end_state(describe: &'static str, probe: &'static str) -> host_channel::PostCondition {
    host_channel::PostCondition {
        describe,
        probe: probe.to_string(),
    }
}

/// The end state an unprivileged restart of a system LaunchDaemon intends.
///
/// A system daemon's job lives in launchd's `system` domain, which an
/// unprivileged login cannot read: `launchctl print system/<label>` needs
/// root, and the `sudo -n` this channel would need is not granted. So
/// [`RUNNING_DESCRIBE`]'s two facts — a loaded job with a pid — are not
/// observable here at all, and asserting them would report every successful
/// restart of a daemon as a failure.
///
/// What IS observable without privilege is the process: it runs as the
/// approved user, so this login can see its pid and its owner. The end state
/// is therefore stated about the process, and it is the honest one for this
/// operation — the whole point of ending a `KeepAlive` daemon's process is
/// that launchd puts a NEW one in its place.
const RESPAWNED_DESCRIBE: &str = "the system daemon is running under a new pid";

/// Reads `daemon_argv` and `daemon_before`, which [`DAEMON_TERM_BODY`] sets.
/// The probe is armed as an `EXIT` trap in the body's own shell
/// (`host_channel::PostCondition::arm`), so it observes the pids that body
/// actually signalled rather than a second, racing observation of its own.
///
/// It asks about the pids running the unit's whole declared argv, not the
/// pids running its program: on a host where every service runs one binary
/// the second question answers with every service, and this probe reported a
/// met end state over the siblings a restart had ended.
const RESPAWNED_PROBE: &str = "  pc_now=$(stado_unit_pids \"${daemon_argv:-}\")
  pc_new=''
  for pc_pid in $pc_now; do
    case \" ${daemon_before:-} \" in
      *\" $pc_pid \"*) ;;
      *) pc_new=\"$pc_new$pc_pid \" ;;
    esac
  done
  if [ -n \"$pc_new\" ]; then
    stado_post 'met' \"$unit runs as pid(s) ${pc_new% }\"
  elif [ -n \"$pc_now\" ]; then
    stado_post 'unmet' \"$unit still runs as the pid(s) this restart ended: $pc_now\"
  else
    stado_post 'unmet' \"nothing runs the program of $unit; launchd did not respawn it\"
  fi
";

/// What the host reports about a system LaunchDaemon, read without
/// privilege and without touching anything.
///
/// Four facts, and each one is a gate on the only repair this channel can
/// perform:
///
/// - the unit's `KeepAlive` spelling, because ending a process nothing will
///   respawn turns a degraded control plane into a dead one;
/// - the account this login runs as;
/// - the pids running exactly the argv the unit declares, that THIS account
///   owns, which are the only ones an unprivileged signal can reach;
/// - the pids running it that some other account owns, so a refusal can say
///   whose process it is instead of just "no".
const DAEMON_PROBE_BODY: &str = "if [ ! -f \"$unit_path\" ]; then
  say 'missing' \"$unit_path\"
  exit 0
fi
daemon_argv=$(stado_unit_argv \"$unit_path\")
daemon_program=\"${daemon_argv%% *}\"
# `raw` answers the scalar spellings (`<true/>`, `<false/>`) in one word.
# A KeepAlive dict has no raw spelling and makes plutil fail, which reads
# identically to a key that is not there -- so the second read asks whether
# the key exists at all, and the third separates an unreadable plist from a
# readable one with no KeepAlive. Three answers, three different repairs.
daemon_keep='absent'
if daemon_raw=$(/usr/bin/plutil -extract KeepAlive raw -o - \"$unit_path\" 2>/dev/null); then
  daemon_keep=$(printf '%s' \"$daemon_raw\" | /usr/bin/tr -d ' \t\r\n')
elif /usr/bin/plutil -extract KeepAlive xml1 -o - \"$unit_path\" >/dev/null 2>&1; then
  daemon_keep='conditional'
elif ! /usr/bin/plutil -lint \"$unit_path\" >/dev/null 2>&1; then
  daemon_keep='unreadable'
fi
if [ -z \"$daemon_keep\" ]; then daemon_keep='unreadable'; fi
daemon_user=$(/usr/bin/id -un)
daemon_owned=''
daemon_foreign=''
# launchd's own answer first, where the domain can be read at all: the pid
# under the label is the one fact no pattern can widen. `sudo -n launchctl
# print system/<label>` is refused on this channel, so a system daemon is
# matched on the argv its unit declares -- never on the program alone, which
# on this fleet names every other service running the same binary.
stado_launchd_state
daemon_pids=\"$pc_pid\"
if [ -z \"$daemon_pids\" ]; then daemon_pids=$(stado_unit_pids \"$daemon_argv\"); fi
for daemon_pid in $daemon_pids; do
  daemon_owner=$(/bin/ps -o user= -p \"$daemon_pid\" 2>/dev/null | /usr/bin/tr -d ' \t\r\n')
  if [ \"$daemon_owner\" = \"$daemon_user\" ]; then
    daemon_owned=\"$daemon_owned$daemon_pid \"
  elif [ -n \"$daemon_owner\" ]; then
    daemon_foreign=\"$daemon_foreign$daemon_pid \"
  fi
done
printf 'STADO_DAEMON\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$daemon_keep\" \"$daemon_user\" \"${daemon_owned% }\" \"${daemon_foreign% }\" \"$daemon_argv\"
say 'daemon_probed' \"KeepAlive $daemon_keep\"
";

/// End the daemon's process so launchd recreates it.
///
/// This is `launchctl kickstart -k` without the privilege: that verb stops
/// the job's process and lets launchd start it again, and for a job launchd
/// is unconditionally keeping alive, ending the process from the account
/// that owns it produces the same sequence. It never unloads anything, so
/// there is no window in which the job does not exist -- the property the
/// July outage cost this fleet three commands to learn.
///
/// Only the pids the probe found under THIS account are signalled, and they
/// arrive as a validated digit list from [`validate_pid_list`]; nothing here
/// re-derives a target from a pattern, because a pattern that widened by one
/// character would signal a process nobody chose.
///
/// TERM only, and no escalation. A control-plane daemon that ignores TERM is
/// a finding to report, not a reason to try SIGKILL on the process holding
/// the fleet's authorization state.
const DAEMON_TERM_BODY: &str = "daemon_argv=@ARGV@
daemon_before=@PIDS@
for daemon_pid in $daemon_before; do /bin/kill -TERM \"$daemon_pid\" >/dev/null 2>&1 || true; done
daemon_after=''
daemon_fresh=''
daemon_waited=0
while [ \"$daemon_waited\" -lt 15 ]; do
  /bin/sleep 1
  daemon_waited=$((daemon_waited + 1))
  daemon_after=$(stado_unit_pids \"$daemon_argv\")
  daemon_fresh=''
  for daemon_pid in $daemon_after; do
    case \" $daemon_before \" in
      *\" $daemon_pid \"*) ;;
      *) daemon_fresh=\"$daemon_fresh$daemon_pid \" ;;
    esac
  done
  if [ -n \"$daemon_fresh\" ]; then break; fi
done
daemon_left=''
for daemon_pid in $daemon_before; do
  case \" $daemon_after \" in
    *\" $daemon_pid \"*) daemon_left=\"$daemon_left$daemon_pid \" ;;
  esac
done
if [ -n \"$daemon_fresh\" ]; then
  say 'restarted' \"ended pid(s) $daemon_before owned by $(/usr/bin/id -un); launchd's KeepAlive replaced it with pid(s) ${daemon_fresh% } after ${daemon_waited}s\"
  exit 0
fi
if [ -n \"$daemon_left\" ]; then
  say 'restart_failed' \"pid(s) ${daemon_left% } did not end on SIGTERM and nothing was unloaded. Run: sudo launchctl kickstart -k system/$unit\"
  exit 0
fi
say 'restart_failed' \"ended pid(s) $daemon_before and launchd started nothing in ${daemon_waited}s. Run: sudo launchctl kickstart -k system/$unit\"
";

/// `service restart`: restart the unit, in place wherever launchd will allow it.
/// Deliberately narrower than a recovery pass — no disk cleanup, no
/// coordinator teardown, no other agents touched.
///
/// Order matters more than it looks. This used to `bootout` first and then
/// `bootstrap` the unit file back, which recreates the job rather than
/// restarting it. When launchd still holds children of the old job the
/// bootstrap fails, and the command returned `restart_failed` with the unit
/// left *unloaded* — a partial failure strictly worse than never having run
/// the restart, because the listeners it owned are gone and there is nothing
/// to roll back to. A control-plane primitive whose failure mode is an outage
/// will eventually cause one; this one did, on the always-on host.
///
/// So a loaded job is kicked in place first: `kickstart -k` never unloads, so
/// there is no window in which the job does not exist and nothing orphaned to
/// sweep. The unload-and-recreate path remains for a unit that is not loaded
/// at all, or whose in-place kick fails.
///
/// What is deliberately NOT here any more is everything that used to happen
/// after the bootstrap failed: a second `launchctl asuser` attempt, a
/// `launchctl submit` of a `<label>-recovery` job, and finally a `perl`-exec
/// of the unit's argv in the background, reported as
/// `restarted: direct process <pid>`. On 2026-08-19 that last line is what
/// `stado service restart com.wisent.compute.service.stado-agent-mini --host
/// control-host` returned, beside `postcondition unmet: no job at
/// user/501/com.wisent.compute.service.stado-agent-mini` — a bare process
/// under the ssh session, no unit behind it, and a report an operator read as
/// success. A process that dies with the login that spawned it is not a
/// restarted service, so a bootstrap that leaves no job in the domain the
/// restart used is [`STATUS_NOT_LOADED`]: the domain, launchd's own words and
/// the reason, and nothing started outside launchd.
const RESTART_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  if [ \"${stado_reload_unit:-0}\" != 1 ] && $launch print \"$domain/$unit\" >/dev/null 2>&1; then
    # An in-place kick re-execs the argv launchd already holds. It cannot
    # apply a unit file whose program or arguments have changed, and it
    # reports success either way -- which is how two restarts and an ensure
    # of com.wisent.compute.service.stado-local-control-plane on 2026-09-03
    # all said `restarted` while the job kept executing the shared global
    # binary the plist no longer named. A silent no-op is the worst available
    # answer, so the two vectors are compared first and a job whose argv has
    # drifted from its file goes to the unload-and-bootstrap path below, which
    # is the only one that can carry the change.
    loaded_argv=$($launch print \"$domain/$unit\" 2>/dev/null | /usr/bin/awk '
      /^[ \\t]*arguments[ \\t]*=[ \\t]*\\{/ { collecting=1; argv=\"\"; next }
      collecting && /^[ \\t]*\\}/ { collecting=0; sub(/^ /, \"\", argv); print argv; exit }
      collecting { line=$0; sub(/^[ \\t]+/, \"\", line); argv=argv \" \" line }
    ')
    file_argv=\"\"
    if [ -f \"$unit_path\" ]; then
      file_argv=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' \"$unit_path\" 2>/dev/null | /usr/bin/sed -e '1d' -e '$d' -e 's/^ *//' | /usr/bin/grep -v '^$' | /usr/bin/tr '\\n' ' ')
      file_argv=$(printf '%s' \"$file_argv\" | /usr/bin/sed -e 's/ *$//')
      [ -n \"$file_argv\" ] || file_argv=$(/usr/libexec/PlistBuddy -c 'Print :Program' \"$unit_path\" 2>/dev/null)
    fi
    loaded_argv=$(printf '%s' \"$loaded_argv\" | /usr/bin/sed -e 's/^ *//' -e 's/ *$//')
    if [ -z \"$file_argv\" ] || [ \"$loaded_argv\" = \"$file_argv\" ]; then\n      detail=$($launch kickstart -k \"$domain/$unit\" 2>&1)\n      rc=$?\n      if [ \"$rc\" -eq 0 ]; then\n        say 'restarted' \"$domain in place\"\n        exit 0\n      fi\n    fi\n  fi
  if [ ! -f \"$unit_path\" ]; then
    $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
    detail=$($launch kickstart -k \"$domain/$unit\" 2>&1)
    rc=$?
    if [ \"$rc\" -eq 0 ]; then say 'restarted' \"$domain\"; else say 'restart_failed' \"$rc $detail\"; fi
    exit 0
  fi
  $launch bootout \"$domain/$unit\" >/dev/null 2>&1 || true
@DISOWNED_SWEEP@
  $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
  detail=$($launch bootstrap \"$domain\" \"$unit_path\" 2>&1)
  rc=$?
  if ! $launch print \"$domain/$unit\" >/dev/null 2>&1; then
    # What the sweep ended goes first. A restart that could not load the unit
    # AND ended the process that was serving without one leaves the host with
    # nothing running this unit, and an operator who is not told that reads the
    # refusal as \"nothing happened\". The unit and the domain are not repeated
    # here: the report names both already.
    say 'not_loaded' \"${left:+ended disowned process(es) $left; }${detail:-launchctl bootstrap said nothing and left no job}\"
    exit 0
  fi
  if [ -n \"$still\" ]; then
    say 'restart_failed' \"disowned process survived: $still; unit reloaded in $domain\"
    exit 0
  fi
  say 'restarted' \"$domain\"
  exit 0
else
  # A user unit must outlive the login session that restarted it. A system
  # unit belongs to the machine manager and needs no per-user linger state.
  if [ \"$scope\" = \"user\" ]; then
    /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || \"$sudo_bin\" -n /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || true
  fi
  stado_systemctl daemon-reload >/dev/null 2>&1 || true
  detail=$(stado_systemctl restart \"$unit\" 2>&1)
  rc=$?
  if [ \"$rc\" -eq 0 ]; then say 'restarted' \"$systemd_detail\"; else say 'restart_failed' \"$rc $detail\"; fi
fi
";

/// `service show`: what the unit FILE declares — its program and argument
/// vector — and nothing about whether any of it is running.
///
/// The status word is `declares`, not `runs`, and the difference is a
/// multi-day outage. This body reaches no process table and asks launchd
/// nothing; it read `ProgramArguments` out of the plist and then said `runs`,
/// so on 2026-08-30 it reported `com.wisent.always-on.weles` as `runs` while
/// both pids the preceding restart had reported were already gone and the
/// unit's stderr ended in `EADDRINUSE`. A word that means "this file exists
/// and declares this" must not be spelled like a word that means "this is
/// serving". Whether the unit is the process on its own port is
/// [`super::service_serving`]'s question.
const SHOW_BODY: &str = "if [ ! -f \"$unit_path\" ]; then
  say 'missing' \"$unit_path\"
  exit 0
fi
if [ \"$os\" = \"Darwin\" ]; then
  args=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' \"$unit_path\" 2>/dev/null | /usr/bin/sed -n '/^[[:space:]]*[^A-Z}]/{s/^[[:space:]]*//;s/[[:space:]]*$//;p;}' | /usr/bin/tr '\\n' ' ')
  if [ -z \"$args\" ]; then args=$(/usr/libexec/PlistBuddy -c 'Print :Program' \"$unit_path\" 2>/dev/null); fi
else
  args=$(/usr/bin/sed -n 's/^ExecStart=//p' \"$unit_path\" | /usr/bin/tr '\\n' ' ')
fi
# A unit that runs .../services/NAME/current/... names a link, not a version,
# and the link is what every rollback and every competing operator moves. The
# declared path therefore stays identical while the code under it changes,
# which makes 'what does this unit run' unanswerable from the unit alone --
# and answering it by guessing has ended badly enough to be worth one readlink.
program=\"${args%% *}\"
resolved=\"\"
case \"$program\" in
  */current/*)
    link=\"${program%%/current/*}/current\"
    if [ -L \"$link\" ]; then resolved=$(/usr/bin/readlink \"$link\"); fi
    ;;
esac
if [ -n \"$resolved\" ]; then
  say 'declares' \"$args(current -> $resolved)\"
else
  say 'declares' \"$args\"
fi
";

/// End a program that outlived every label it was ever started under.
///
/// Booting out a label is not the same as the program being gone. A unit started
/// once outside its own label -- by a recovery fallback, or by hand -- survives
/// every bootout, keeps the listening socket, and makes each later start die on
/// `address already in use` while the stale instance serves on.
///
/// `stop` has always done this. `restart` did not, which is why a restart after
/// `service update` reported success and left the previous version serving: the
/// relink took effect on no restart at all, and the operator had to know to stop
/// first. Both bodies now splice in this one sweep, so they cannot disagree about
/// what stopping means.
///
/// Sets `left` (what was found) and `still` (what survived a TERM); reporting is
/// the caller's, because stop and restart have different things to say about it.
///
/// Scoped to the unit's whole declared argv, through `stado_unit_pids`. It used
/// to sweep every process whose executable was the unit's program, and on a host
/// where one binary runs the object API, the resolver, the agent and the beacon
/// that is a sweep of the control plane: `stado service restart
/// com.wisent.always-on.stado-object-api --host control-host` on
/// 2026-08-19 TERMed eight processes, among them the host's resolver holding
/// 17600/17601/17612/17621, and reported one unit restarted.
const DISOWNED_SWEEP: &str = "  sweep_argv=$(stado_unit_argv \"$unit_path\")
  left=\"\"
  still=\"\"
  if [ -n \"$sweep_argv\" ]; then
    left=$(stado_unit_pids \"$sweep_argv\")
    if [ -n \"$left\" ]; then
      for pid in $left; do /bin/kill -TERM \"$pid\" >/dev/null 2>&1 || true; done
      /bin/sleep 2
      still=$(stado_unit_pids \"$sweep_argv\")
      # A service that serves each adapter from its own process does not go
      # away on one round of TERM: the process holding the port exits, the
      # siblings holding theirs do not, and launchd is then refused the ports
      # it is being asked to bind. Reporting that as \"survived\" left the unit
      # booted out -- a restart that ends with nothing running. Escalate, and
      # keep 'survived' for a process that refuses SIGKILL.
      if [ -n \"$still\" ]; then
        for pid in $still; do /bin/kill -KILL \"$pid\" >/dev/null 2>&1 || true; done
        /bin/sleep 2
        still=$(stado_unit_pids \"$sweep_argv\")
      fi
    fi
  fi
";

/// `service stop`: boot the label out of the domain the resolver chose, then end
/// whatever is still running the unit's argv.
///
/// The second bootout is cleanup and not a second opinion about where the unit
/// lives: the resolution this fix replaces bootstrapped agents into whichever
/// per-login domain answered first, so a host can still carry the job under the
/// spelling the resolver did not choose, and a stop that left that one loaded
/// would be a fence with a writer behind it. Every report and every end-state
/// probe names `$domain`.
const STOP_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  recovery_unit=\"${unit}-recovery\"
  other_domain=\"\"
  case \"$domain\" in
    \"$gui\") other_domain=\"$user_domain\" ;;
    \"$user_domain\") other_domain=\"$gui\" ;;
  esac
  $launch bootout \"$domain/$unit\" >/dev/null 2>&1 || true
  $launch bootout \"$domain/$recovery_unit\" >/dev/null 2>&1 || true
  if [ -n \"$other_domain\" ]; then
    /bin/launchctl bootout \"$other_domain/$unit\" >/dev/null 2>&1 || true
    /bin/launchctl bootout \"$other_domain/$recovery_unit\" >/dev/null 2>&1 || true
  fi
@DISOWNED_SWEEP@
  if [ -n \"$left\" ]; then
    if [ -n \"$still\" ]; then
      say 'stop_failed' \"disowned process still running: $still\"
      exit 0
    fi
    say 'stopped' \"booted out of $domain, and ended disowned process(es): $left\"
    exit 0
  fi
else
  stado_systemctl stop \"$unit\" >/dev/null 2>&1 || true
fi
say 'stopped' \"$unit_path\"
";

/// `service adopt`: a read-only probe. Adoption claims an existing unit, so
/// the host has to agree the unit is there before the registry says Stado
/// owns it — that check is the whole difference between adoption and
/// fiction.
const PROBE_BODY: &str = "file_state='absent'
if [ -f \"$unit_path\" ]; then file_state='present'; fi
unit_state='unloaded'
if [ \"$os\" = \"Darwin\" ]; then
  if /bin/launchctl print \"$domain/$unit\" >/dev/null 2>&1; then unit_state='loaded'; fi
else
  if stado_systemctl cat \"$unit\" >/dev/null 2>&1; then unit_state='loaded'; fi
fi
printf 'STADO_ADOPT\\t%s\\t%s\\n' \"$file_state\" \"$unit_state\"
say 'probed' \"$unit_path\"
";

/// `service retire`: withdraw and stop, while leaving the unit file on disk.
///
/// launchd has two per-login spellings, so both are booted out and disabled.
/// systemd is runtime-masked before it is disabled. The mask is the rolling
/// upgrade fence: a coordinator still running an older Stado may have read the
/// declaration before its withdrawal and try one last `enable --now`; systemd
/// must refuse that stale start without relying on the new shared lease.
const RETIRE_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  recovery_unit=\"${unit}-recovery\"
  /bin/launchctl bootout \"$gui/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$user_domain/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$gui/$recovery_unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$user_domain/$recovery_unit\" >/dev/null 2>&1 || true
  /bin/launchctl disable \"$gui/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl disable \"$user_domain/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl disable \"$gui/$recovery_unit\" >/dev/null 2>&1 || true
  /bin/launchctl disable \"$user_domain/$recovery_unit\" >/dev/null 2>&1 || true
  say 'retired' \"$unit_path\"
else
  stado_systemctl disable --now \"$unit\" >/dev/null 2>&1 || true
  detail=$(stado_systemctl mask --runtime --now \"$unit\" 2>&1)
  rc=$?
  if [ \"$rc\" -ne 0 ]; then
    say 'retire_failed' \"$rc $detail\"
  elif stado_systemctl is-active --quiet \"$unit\"; then
    say 'retire_failed' \"$unit remained active after its runtime mask\"
  else
    say 'retired' \"$unit_path\"
  fi
fi
";

/// `service deploy`: write the rendered unit, then bootstrap it. Both
/// renderings travel in the same program and the host picks, so a deploy
/// costs one round trip and never depends on a local guess about the
/// remote OS.
const DEPLOY_BODY: &str = "program=@PROGRAM@
if [ ! -f \"$program\" ]; then
  say 'program_missing' \"$program\"
  exit 0
fi
if [ ! -x \"$program\" ]; then
  /bin/chmod u+x \"$program\" || {
    say 'program_not_executable' \"$program\"
    exit 0
  }
fi
if [ \"$os\" = \"Darwin\" ]; then
  /bin/mkdir -p \"$HOME/Library/LaunchAgents\" \"$HOME/.stado/logs\" >/dev/null 2>&1 || exit 1
  /bin/chmod u=rwx,go= \"$HOME/.stado/logs\" || exit 1
  log=\"$HOME/.stado/logs/$unit.log\"
  : >> \"$log\" || exit 1
  /bin/chmod u=rw,go= \"$log\" || exit 1
  template=\"$unit_path.template.$$\"
  /bin/cat > \"$template\" <<'@HEREDOC@'
@DARWIN_UNIT@
@HEREDOC@
  escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
  /usr/bin/sed \"s/__STADO_HOME__/$escaped_home/g\" \"$template\" > \"$unit_path\" || exit 1
  /bin/rm -f \"$template\"
  /bin/chmod u=rw,go= \"$unit_path\" || exit 1
  $launch bootout \"$domain/$unit\" >/dev/null 2>&1 || true
  detail=$($launch bootstrap \"$domain\" \"$unit_path\" 2>&1)
  rc=$?
  # No `asuser` retry into a domain this login cannot join, no `launchctl
  # submit` of a second label, and no `nohup` of the program: a deploy is
  # recorded in the canonical registry by its caller, and a record naming a
  # unit launchd never accepted is a declaration no later command can act on.
  if ! $launch print \"$domain/$unit\" >/dev/null 2>&1; then
    say 'not_loaded' \"${detail:-launchctl bootstrap said nothing and left no job}\"
    exit 0
  fi
  $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
  $launch kickstart -k \"$domain/$unit\" >/dev/null 2>&1 || true
  say 'deployed' \"$unit_path\"
else
  /bin/mkdir -p \"$HOME/.config/systemd/user\" >/dev/null 2>&1 || true
  template=\"$unit_path.template.$$\"
  /bin/cat > \"$template\" <<'@HEREDOC@'
@LINUX_UNIT@
@HEREDOC@
  escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
  account=$(/usr/bin/id -un)
  /usr/bin/sed -e \"s/__STADO_HOME__/$escaped_home/g\" -e \"s/__STADO_USER__/$account/g\" \"$template\" > \"$unit_path\" || exit 1
  /bin/rm -f \"$template\"
  /bin/chmod u=rw,go= \"$unit_path\" || exit 1
  # A user unit lives inside the user's systemd instance, and without linger
  # that instance ends with the login session that created it — on rtx every
  # user-scoped service (beacon, agent, router) died seconds after the deploy
  # channel closed, and the host read as down. A system unit belongs to the
  # machine manager and needs no per-user linger state.
  if [ \"$scope\" = \"user\" ]; then
    /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || \"$sudo_bin\" -n /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || true
  fi
  stado_systemctl daemon-reload >/dev/null 2>&1 || true
  stado_systemctl unmask \"$unit\" >/dev/null 2>&1 || true
  detail=$(stado_systemctl enable --now \"$unit\" 2>&1)
  rc=$?
  if [ \"$rc\" -eq 0 ]; then say 'deployed' \"$unit_path\"; else say 'enable_failed' \"$rc $detail\"; fi
fi
";

/// `service ensure`: the unit this host should be running, installed only
/// where it is not already what it should be.
///
/// Three differences from [`DEPLOY_BODY`], each one an incident:
///
/// - It is idempotent. `deploy` refuses a unit that is already declared and
///   bootstraps unconditionally otherwise, so there is no command an operator
///   can run twice, or run from a script, to assert what a host must be
///   running. This one reads what is there first and reports
///   `already_correct` having touched nothing.
/// - It installs into the domain that exists. The prelude's
///   [`NO_DOMAIN_SYSTEM`] fallback has already chosen
///   `/Library/LaunchDaemons` on an ssh login with no Aqua session, which is
///   the case `deploy` fails on with `Could not switch to audit session ...
///   Operation not permitted`, having installed nothing.
/// - It compares the plist, launchd's retained Program and argument vector,
///   and the running executable. A differing retained definition is reloaded
///   only after executable and rendered-unit preflight, with a genuinely
///   distinct prior unit restored if activation fails and launchd's readback
///   verified on success.
///
/// There is deliberately no fallback to `launchctl submit` or to a bare
/// background process. Those two are how a host comes to run a program no
/// unit owns, which is the state `list --unowned` exists to find and this
/// command exists to end.
const ENSURE_BODY: &str = "program=@PROGRAM@
argv=@ARGV@
# The staged unit is removed inline, on every path, and NO `trap` is installed
# for it. `host_channel::PostCondition::arm` arms the end-state probe as an EXIT
# trap before this body runs, and a second `trap ... EXIT` here replaces it: a
# create pass then wrote the plist, bootstrapped it, left launchd running it
# with a live pid, and still failed with `postcondition unobserved`, because the
# probe that would have confirmed the success had been unhooked by the cleanup.
staged=''
stado_loaded_identity() {
  loaded_program=$(printf '%s\\n' \"$pc_info\" | /usr/bin/awk -F' = ' '$1 ~ /^[[:space:]]*program[[:space:]]*$/ { print $2; exit }')
  loaded_arguments_rc=0
  loaded_argv=$(printf '%s\\n' \"$pc_info\" | /usr/bin/awk '
    /^[[:space:]]*arguments[[:space:]]*=[[:space:]]*\\{/ { seen=1; collecting=1; argv=\"\"; next }
    collecting && /^[[:space:]]*\\}/ { complete=1; sub(/^ /, \"\", argv); print argv; exit }
    collecting { line=$0; sub(/^[[:space:]]+/, \"\", line); sub(/[[:space:]]+$/, \"\", line); if (line != \"\") argv = argv \" \" line }
    END { if (!seen) exit 3; if (!complete) exit 4 }') || loaded_arguments_rc=$?
  loaded_program=$(printf '%s' \"$loaded_program\" | /usr/bin/sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
  loaded_arguments_valid=yes
  case \"$loaded_arguments_rc\" in
    0) loaded_argv=$(printf '%s' \"$loaded_argv\" | /usr/bin/tr -s ' ' | /usr/bin/sed 's/^ //;s/ $//') ;;
    3) loaded_argv=\"$loaded_program\" ;;
    *) loaded_arguments_valid=no; loaded_argv='' ;;
  esac
}
# Whether the process launchd or systemd reports under this unit executes the
# declared program. Sets `running` and `serves`.
#
# `comm` is the image the kernel runs, and for a program that is a launcher it
# is the launcher's exec target: `bin/start-web` execs node, so every web unit
# reports `node` and equality with the program fails for all of them. On
# 2026-09-05 that refused the reload of two running sites with `pid 6678
# executes [node]; expected [.../current/darwin-arm/bin/start-web]` and
# restarted every healthy web unit on each ensure pass, because the idle check
# read the same `no`. A launcher's process still runs the product: its image,
# an argument, or its working directory lies under the product root that the
# `current` link belongs to. That is the evidence accepted here, and it is one
# rule for the idle check and the post-reload verification, so one process
# cannot be `already_correct` before a reload and a verification failure after
# it. A program outside a `current` tree keeps the exact comparison: the
# control-plane job that went on executing the shared global binary its plist
# no longer named is the case that comparison exists for.
stado_process_serves() {
  running=''
  serves=no
  [ -n \"$1\" ] || return 0
  running=$(/bin/ps -p \"$1\" -o comm= 2>/dev/null)
  case \"$running\" in
    \"$program\") serves=yes; return 0 ;;
  esac
  case \"$program\" in
    */current/*) ;;
    *) return 0 ;;
  esac
  product_root=\"${program%%/current/*}/\"
  case \"$running\" in
    \"$product_root\"*) serves=yes; return 0 ;;
  esac
  command=$(/bin/ps -p \"$1\" -o command= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
  case \" $command\" in
    *\" $product_root\"*|*\"=$product_root\"*) serves=yes; return 0 ;;
  esac
  if [ \"$os\" = Darwin ]; then
    cwd=$(/usr/sbin/lsof -a -p \"$1\" -d cwd -Fn 2>/dev/null | /usr/bin/sed -n 's/^n//p' | /usr/bin/head -n 1)
  else
    cwd=$(/usr/bin/readlink \"/proc/$1/cwd\" 2>/dev/null)
  fi
  case \"$cwd/\" in
    \"$product_root\"*) serves=yes ;;
  esac
}
bail() {
  if [ -n \"$staged\" ]; then /bin/rm -f \"$staged\" \"$staged.rendered\"; fi
  say 'ensure_failed' \"$1\"
  exit 0
}
if [ ! -f \"$program\" ]; then
  say 'program_missing' \"$program\"
  exit 0
fi
if [ ! -x \"$program\" ]; then
  /bin/chmod u+x \"$program\" || {
    say 'program_not_executable' \"$program\"
    exit 0
  }
fi
# What the unit on disk says it runs, as the one-line spelling `plan_deploy`
# renders, so 'the unit already runs this' is a comparison of two spellings of
# one list rather than a guess.
#
# Exactly PlistBuddy's own array framing is dropped, by matching those two
# lines. `service show`'s character-class filter keeps the arguments only
# because PlistBuddy indents them, and the whole of this command turns on the
# comparison being exact: a readback that lost one argument would make a unit
# this command installed itself read as declaring something else on the very
# next run, and the answer would be `conflict` forever.
declared_argv=''
had_unit=no
if [ \"$os\" = \"Darwin\" ]; then
  if [ -f \"$unit_path\" ]; then
    declared_argv=$(stado_unit_argv \"$unit_path\")
  fi
  stado_launchd_state
  had_unit=\"$pc_loaded\"
  pid=\"$pc_pid\"
  stado_loaded_identity
else
  if [ -f \"$unit_path\" ]; then
    had_unit=yes
    declared_argv=$(/usr/bin/sed -n 's/^ExecStart=//p' \"$unit_path\" | /usr/bin/head -n 1)
  fi
  pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
  if [ \"$pid\" = 0 ]; then pid=''; fi
fi
declared_argv=$(printf '%s' \"$declared_argv\" | /usr/bin/tr -s ' ' | /usr/bin/sed 's/^ //;s/ $//')
# The program the live process is executing, not the one the unit names: a
# unit pointing at a `current` link and a process that outlived the relink
# have the same declaration and different code.
stado_process_serves \"$pid\"
# Compare the whole desired unit, including its environment, on both init
# systems. A loaded launchd definition can outlive a removed plist, but the
# desired declaration is still complete enough to render and safely reload it.
rendered=''
if [ -f \"$unit_path\" ] || { [ \"$os\" = Darwin ] && [ \"$had_unit\" = yes ]; }; then
  staged=\"$HOME/.stado/$unit.ensure.$$\"
  if [ \"$os\" = Linux ]; then
    /bin/cat > \"$staged\" <<'@HEREDOC@'
@LINUX_UNIT@
@HEREDOC@
  elif [ \"$domain\" = system ]; then
    /bin/cat > \"$staged\" <<'@HEREDOC@'
@DARWIN_DAEMON_UNIT@
@HEREDOC@
  else
    /bin/cat > \"$staged\" <<'@HEREDOC@'
@DARWIN_UNIT@
@HEREDOC@
  fi
  escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
  account=$(/usr/bin/id -un)
  /usr/bin/sed -e \"s/__STADO_HOME__/$escaped_home/g\" -e \"s/__STADO_USER__/$account/g\" \"$staged\" > \"$staged.rendered\" || bail 'cannot render the unit'
  rendered=\"$staged.rendered\"
  if [ \"$os\" = Darwin ]; then
    rc=0
    detail=$(/usr/bin/plutil -lint \"$rendered\" 2>&1) || rc=$?
    if [ \"$rc\" -ne 0 ]; then bail \"plutil preflight exited $rc: ${detail:-no detail}\"; fi
  fi
fi
stado_install_unit() {
  if [ \"$os\" = Darwin ] && [ \"$domain\" = system ]; then
    /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel \"$1\" \"$unit_path.stado-ensure.$$\" \
      && /usr/bin/sudo -n /bin/mv -f \"$unit_path.stado-ensure.$$\" \"$unit_path\"
  elif [ \"$os\" = Linux ] && [ \"$scope\" = system ]; then
    stado_root /usr/bin/install -m 644 -o root -g root \"$1\" \"$unit_path.stado-ensure.$$\" \
      && stado_root /bin/mv -f \"$unit_path.stado-ensure.$$\" \"$unit_path\"
  else
    /bin/cp \"$1\" \"$unit_path.stado-ensure.$$\" \
      && /bin/chmod u=rw,go= \"$unit_path.stado-ensure.$$\" \
      && /bin/mv -f \"$unit_path.stado-ensure.$$\" \"$unit_path\"
  fi
}
stado_activate_definition() {
  activation_failure=''
  if [ \"$os\" = Darwin ]; then
    stado_launchd_state
    if [ \"$pc_loaded\" = yes ]; then
      activation_detail=$($launch bootout \"$domain/$unit\" 2>&1)
      activation_rc=$?
      if [ \"$activation_rc\" -ne 0 ]; then
        activation_failure=\"launchctl bootout exited $activation_rc: ${activation_detail:-no detail}\"
        return 1
      fi
      attempts=0
      while $launch print \"$domain/$unit\" >/dev/null 2>&1; do
        attempts=$((attempts + 1))
        if [ \"$attempts\" -ge 150 ]; then
          activation_failure=\"launchctl bootout exited 0 but $domain/$unit remained loaded\"
          return 1
        fi
        /bin/sleep 0.1
      done
    fi
    # A disabled service is what `stado service stop` and the release agent's
    # `stop_legacy` leave behind, and `bootstrap` refuses it with `Bootstrap
    # failed: 5: Input/output error` - the create path below already enables
    # before it bootstraps, and this path did not. On 2026-09-06 that refused
    # the one command that could give charless-mac-mini its Skarbiec unit back
    # after the release path abandoned the stable bind, and then failed the
    # rollback with the same error, thirteen hours into an outage.
    $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
    activation_detail=$($launch bootstrap \"$domain\" \"$unit_path\" 2>&1)
    activation_rc=$?
    if [ \"$activation_rc\" -ne 0 ]; then
      activation_failure=\"launchctl bootstrap exited $activation_rc: ${activation_detail:-no detail}\"
      return 1
    fi
  else
    activation_detail=$(stado_systemctl daemon-reload 2>&1)
    activation_rc=$?
    if [ \"$activation_rc\" -ne 0 ]; then
      activation_failure=\"systemctl daemon-reload exited $activation_rc: ${activation_detail:-no detail}\"
      return 1
    fi
    activation_detail=$(stado_systemctl restart \"$unit\" 2>&1)
    activation_rc=$?
    if [ \"$activation_rc\" -ne 0 ]; then
      activation_failure=\"systemctl restart exited $activation_rc: ${activation_detail:-no detail}\"
      return 1
    fi
  fi
  attempts=0
  while [ \"$attempts\" -lt 150 ]; do
    if [ \"$os\" = Darwin ]; then
      stado_launchd_state
      pid=\"$pc_pid\"
    else
      pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
    fi
    if [ -n \"$pid\" ] && [ \"$pid\" != 0 ] && /bin/kill -0 \"$pid\" 2>/dev/null; then
      return 0
    fi
    attempts=$((attempts + 1))
    /bin/sleep 0.1
  done
  activation_failure=\"activation exited 0 but $unit did not acquire a live pid\"
  return 1
}
loaded_drift=no
if [ \"$os\" = Darwin ] && [ \"$had_unit\" = yes ]; then
  if [ -z \"$loaded_program\" ] || [ \"$loaded_arguments_valid\" != yes ]; then
    /bin/rm -f \"$staged\" \"$rendered\"
    say 'loaded_definition_unknown' \"$domain/$unit launchctl readback did not expose a valid Program and complete arguments definition\"
    exit 0
  fi
  if [ \"$loaded_program\" != \"$program\" ] || [ \"$loaded_argv\" != \"$argv\" ]; then
    loaded_drift=yes
  fi
fi
unit_drift=no
if [ -n \"$rendered\" ] && { [ ! -f \"$unit_path\" ] || ! /bin/cmp -s \"$rendered\" \"$unit_path\"; }; then
  unit_drift=yes
fi
reload_needed=no
reload_action=converged
if [ \"$os\" = Darwin ] && [ \"$had_unit\" = yes ] \
  && { [ \"$loaded_drift\" = yes ] || [ \"$unit_drift\" = yes ]; }; then
  reload_needed=yes
  if [ \"$loaded_drift\" = yes ]; then reload_action=reloaded; fi
elif [ \"$declared_argv\" = \"$argv\" ] && [ \"$unit_drift\" = yes ]; then
  reload_needed=yes
fi
if [ \"$reload_needed\" = yes ]; then
  [ -n \"$rendered\" ] || bail 'cannot reload a definition without rendered configuration'
  previous=''
  rollback_unavailable='no prior unit file existed'
  if [ -f \"$unit_path\" ]; then
    if [ \"$unit_drift\" = no ]; then
      rollback_unavailable='existing unit already matched the desired definition; no distinct prior definition exists'
    else
      previous=\"$staged.previous\"
      /bin/cp \"$unit_path\" \"$previous\" || bail 'cannot preserve the prior unit'
      /bin/chmod u=rw,go= \"$previous\" || bail 'cannot protect the prior unit'
      rollback_unavailable=''
    fi
  fi
  if [ \"$unit_drift\" = yes ] && ! stado_install_unit \"$rendered\"; then
    if [ -n \"$previous\" ]; then
      stado_install_unit \"$previous\" || bail \"unit write failed; rollback failed; prior unit is $previous\"
      /bin/rm -f \"$previous\"
      bail 'unit write failed; prior unit restored'
    fi
    bail \"unit write failed; rollback not attempted: $rollback_unavailable\"
  fi
  if ! stado_activate_definition; then
    replacement_failure=\"$activation_failure\"
    if [ -n \"$previous\" ]; then
      if stado_install_unit \"$previous\" && stado_activate_definition; then
        /bin/rm -f \"$previous\"
        bail \"replacement activation failed ($replacement_failure); prior unit restored and running\"
      fi
      bail \"replacement activation failed ($replacement_failure); rollback failed; prior unit is $previous\"
    fi
    bail \"replacement activation failed ($replacement_failure); rollback not attempted: $rollback_unavailable\"
  fi
  verification_failure=''
  if [ \"$os\" = Darwin ]; then
    stado_loaded_identity
    if [ -z \"$loaded_program\" ] || [ \"$loaded_arguments_valid\" != yes ]; then
      verification_failure='launchctl readback after reload did not expose a valid Program and complete arguments definition'
    elif [ \"$loaded_program\" != \"$program\" ] || [ \"$loaded_argv\" != \"$argv\" ]; then
      verification_failure=\"launchctl retained program [$loaded_program] argv [$loaded_argv]; expected program [$program] argv [$argv]\"
    else
      stado_process_serves \"$pid\"
      if [ \"$serves\" != yes ]; then
        verification_failure=\"$domain/$unit pid $pid executes [$running]; expected [$program]\"
      fi
    fi
  fi
  if [ -n \"$verification_failure\" ]; then
    if [ -n \"$previous\" ]; then
      if stado_install_unit \"$previous\" && stado_activate_definition; then
        /bin/rm -f \"$previous\"
        bail \"replacement verification failed ($verification_failure); prior unit restored and running\"
      fi
      bail \"replacement verification failed ($verification_failure); rollback failed; prior unit is $previous\"
    fi
    bail \"replacement verification failed ($verification_failure); rollback not attempted: $rollback_unavailable\"
  fi
  /bin/rm -f \"$previous\" \"$staged\" \"$rendered\"
  printf 'STADO_ENSURE\\t%s\\t%s\\t%s\\n' \"$domain\" \"$pid\" \"$unit_path\"
  say \"$reload_action\" \"$unit_path reloaded and verified\"
  exit 0
fi
if [ \"$declared_argv\" = \"$argv\" ] && [ \"$serves\" = yes ]; then
  /bin/rm -f \"$staged\" \"$rendered\"
  printf 'STADO_ENSURE\\t%s\\t%s\\t%s\\n' \"$domain\" \"$pid\" \"$unit_path\"
  say 'already_correct' \"$domain/$unit pid $pid\"
  exit 0
fi
if [ \"$declared_argv\" = \"$argv\" ]; then
  /bin/rm -f \"$staged\" \"$rendered\"
  rendered=''
fi
if [ \"$declared_argv\" != \"$argv\" ]; then

  if [ \"$os\" = \"Darwin\" ]; then
    /bin/rm -f \"$staged\" \"$rendered\"
    /bin/mkdir -p \"$HOME/.stado/logs\" >/dev/null 2>&1 || bail 'cannot create the log directory'
    /bin/chmod u=rwx,go= \"$HOME/.stado/logs\" || bail 'cannot protect the log directory'
    log=\"$HOME/.stado/logs/$unit.log\"
    : >> \"$log\" || bail \"cannot create $log\"
    /bin/chmod u=rw,go= \"$log\" || bail \"cannot protect $log\"
    staged=\"$HOME/.stado/$unit.plist.$$\"
    if [ \"$domain\" = system ]; then
      /bin/cat > \"$staged\" <<'@HEREDOC@'
@DARWIN_DAEMON_UNIT@
@HEREDOC@
    else
      /bin/mkdir -p \"$HOME/Library/LaunchAgents\" >/dev/null 2>&1 || bail 'cannot create LaunchAgents'
      /bin/cat > \"$staged\" <<'@HEREDOC@'
@DARWIN_UNIT@
@HEREDOC@
    fi
    escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
    account=$(/usr/bin/id -un)
    /usr/bin/sed -e \"s/__STADO_HOME__/$escaped_home/g\" -e \"s/__STADO_USER__/$account/g\" \"$staged\" > \"$staged.rendered\" || bail 'cannot render the unit'
    if [ \"$domain\" = system ]; then
      /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel \"$staged.rendered\" \"$unit_path\" || bail \"sudo -n install $unit_path was refused\"
    else
      /bin/cp \"$staged.rendered\" \"$unit_path\" || bail \"cannot write $unit_path\"
      /bin/chmod u=rw,go= \"$unit_path\" || bail \"cannot protect $unit_path\"
    fi
    /bin/rm -f \"$staged\" \"$staged.rendered\"
  else
    /bin/rm -f \"$staged\" \"$rendered\"
    /bin/mkdir -p \"$HOME/.config/systemd/user\" >/dev/null 2>&1 || bail 'cannot create the systemd user directory'
    staged=\"$HOME/.stado/$unit.ensure.$$\"
    /bin/cat > \"$staged\" <<'@HEREDOC@'
@LINUX_UNIT@
@HEREDOC@
    escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
    account=$(/usr/bin/id -un)
    /usr/bin/sed -e \"s/__STADO_HOME__/$escaped_home/g\" -e \"s/__STADO_USER__/$account/g\" \"$staged\" > \"$staged.rendered\" || bail 'cannot render the unit'
    stado_install_unit \"$staged.rendered\" || bail \"cannot write $unit_path\"
    /bin/rm -f \"$staged\" \"$staged.rendered\"
  fi
fi
if [ \"$os\" = \"Darwin\" ]; then
  if [ \"$had_unit\" = yes ]; then
    action=restarted
    detail=$($launch kickstart -k \"$domain/$unit\" 2>&1)
    rc=$?
  else
    action=created
    $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
    detail=$($launch bootstrap \"$domain\" \"$unit_path\" 2>&1)
    rc=$?
  fi
else
  # Same linger guarantee as DEPLOY_BODY, only for per-user units.
  if [ \"$scope\" = \"user\" ]; then
    /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || \"$sudo_bin\" -n /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || true
  fi
  stado_systemctl daemon-reload >/dev/null 2>&1 || true
  stado_systemctl unmask \"$unit\" >/dev/null 2>&1 || true
  if [ \"$had_unit\" = yes ]; then
    action=restarted
    detail=$(stado_systemctl restart \"$unit\" 2>&1)
    rc=$?
  else
    action=created
    detail=$(stado_systemctl enable --now \"$unit\" 2>&1)
    rc=$?
  fi
fi
if [ \"$rc\" -ne 0 ]; then
  say \"${action}_failed\" \"$rc $detail\"
  exit 0
fi
/bin/sleep 1
pid=''
if [ \"$os\" = \"Darwin\" ]; then
  stado_launchd_state
  if [ \"$pc_loaded\" = no ]; then
    # The verb reported success and launchd has no job under the label: the
    # same shape a `restarted: direct process <pid>` used to hide.
    say 'not_loaded' \"${detail:-launchctl reported success and left no job}\"
    exit 0
  fi
  pid=\"$pc_pid\"
else
  pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
  if [ \"$pid\" = 0 ]; then pid=''; fi
fi
printf 'STADO_ENSURE\t%s\t%s\t%s\n' \"$domain\" \"$pid\" \"$unit_path\"
say \"$action\" \"$unit_path\"
";

/// `service converge`: which artefact the live process is executing, as
/// against the one the unit's declaration resolves to today.
///
/// Read-only. Two production incidents are exactly this gap, and neither is
/// visible in any other answer this group gives: Brama's process kept running
/// an artefact tree that `current` no longer pointed at, and the Weles worker
/// kept serving a `dist` that was replaced 26 seconds after it started. In
/// both cases the unit was loaded, the declaration was true, the version on
/// disk was the declared one, and the running code was not it.
///
/// So the host reports facts and the verdict is computed off-host by
/// [`RunningProgram::matches_process`]: the pid, the program the unit
/// declares, what that declaration's `current` link resolves to now, the
/// executable the process table says the pid is running, when the process
/// started, and when each of those two files was last written. A judgement
/// made in shell would be a second opinion about artefact identity.
const PROCESS_BODY: &str = "declared=''
if [ \"$os\" = \"Darwin\" ]; then
  if [ -f \"$unit_path\" ]; then
    declared=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments:0' \"$unit_path\" 2>/dev/null)
    if [ -z \"$declared\" ]; then
      declared=$(/usr/libexec/PlistBuddy -c 'Print :Program' \"$unit_path\" 2>/dev/null)
    fi
  fi
  stado_launchd_state
  pid=\"$pc_pid\"
else
  if [ -f \"$unit_path\" ]; then
    declared=$(/usr/bin/sed -n 's/^ExecStart=//p' \"$unit_path\" | /usr/bin/head -n 1)
    declared=\"${declared%% *}\"
  fi
  pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
  if [ \"$pid\" = 0 ]; then pid=''; fi
fi
# A unit that runs .../current/... names a link, and the link is what every
# release and every rollback moves. The declaration therefore stays identical
# while the artefact under it changes, so the link has to be resolved here to
# have anything to compare the running process against.
resolved=\"$declared\"
case \"$declared\" in
  */current/*)
    link=\"${declared%%/current/*}/current\"
    leaf=\"${declared#*/current/}\"
    if [ -L \"$link\" ]; then
      dest=$(/usr/bin/readlink \"$link\")
      case \"$dest\" in
        /*) resolved=\"$dest/$leaf\" ;;
        *) resolved=\"${declared%%/current/*}/$dest/$leaf\" ;;
      esac
    fi
    ;;
esac
running=''
started=''
declared_written=''
running_written=''
if [ -n \"$pid\" ]; then
  running=$(/bin/ps -p \"$pid\" -o comm= 2>/dev/null)
  lstart=$(/bin/ps -p \"$pid\" -o lstart= 2>/dev/null)
  if [ \"$os\" = \"Darwin\" ]; then
    started=$(/bin/date -j -f '%a %b %d %T %Y' \"$lstart\" +%s 2>/dev/null)
    if [ -f \"$resolved\" ]; then declared_written=$(/usr/bin/stat -f %m \"$resolved\" 2>/dev/null); fi
    if [ -f \"$running\" ]; then running_written=$(/usr/bin/stat -f %m \"$running\" 2>/dev/null); fi
  else
    started=$(/usr/bin/date -d \"$lstart\" +%s 2>/dev/null)
    if [ -f \"$resolved\" ]; then declared_written=$(/usr/bin/stat -c %Y \"$resolved\" 2>/dev/null); fi
    if [ -f \"$running\" ]; then running_written=$(/usr/bin/stat -c %Y \"$running\" 2>/dev/null); fi
  fi
fi
printf 'STADO_PROCESS\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" \"$declared\" \"$resolved\" \"$running\" \"$started\" \"$declared_written\" \"$running_written\"
say 'inspected' \"$unit\"
";

/// `service list --unowned`: product processes on one host that no unit owns.
///
/// Its own program rather than one more body on [`REMOTE_PRELUDE`], because it
/// addresses no unit: there is nothing to splice, and a sentinel unit id would
/// make the shared prelude derive a plist path for a unit that does not exist.
/// It is read-only in the strongest sense — it starts nothing, stops nothing,
/// signals nothing, and needs no sudo — so it is safe against a live host.
///
/// Two `stado agent` processes ran for four days on the always-on mac with no
/// launchd unit behind them, executing a binary older than the one on disk,
/// and every command in this group answered about declared units and so said
/// nothing at all about them. Ownership is asked of launchd itself: the pids
/// in the `services` table of each printable domain, and any descendant of one
/// of those pids, are owned. On Linux the same question is the cgroup the
/// kernel put the process in — a `.service` cgroup is a unit's, a `.scope` is
/// a login session's.
const UNOWNED_SCRIPT: &str = "set -u
os=$(/usr/bin/uname -s)
uid=$(/usr/bin/id -u)
set -- @ROOTS@
owned=''
owner_of=''
if [ \"$os\" = \"Darwin\" ]; then
  for launchd_domain in \"gui/$uid\" \"user/$uid\" system; do
    owned=\"$owned $(/bin/launchctl print \"$launchd_domain\" 2>/dev/null | /usr/bin/awk '/services = \\{/ { inside = 1; next } inside && /^[[:space:]]*\\}/ { inside = 0 } inside && $1 ~ /^[0-9]+$/ { print $1 }' | /usr/bin/tr '\\n' ' ')\"
  done
  # `owner_of` is set to the pid in the chain that matched, so a verdict of
  # \"owned\" can be checked instead of taken. The whole reason this command
  # answered an empty table for as long as it existed is that nothing printed
  # WHY a candidate was judged owned: launchd claims about a thousand pids on a
  # mac, and against a set that size the test is nearly always true.
  owns() {
    walk=\"$1\"
    owner_of=''
    while [ -n \"$walk\" ] && [ \"$walk\" != 0 ] && [ \"$walk\" != 1 ]; do
      case \" $owned \" in *\" $walk \"*) owner_of=\"$walk\"; return 0 ;; esac
      walk=$(/bin/ps -p \"$walk\" -o ppid= 2>/dev/null | /usr/bin/tr -d ' ')
    done
    return 1
  }
else
  # systemd hosts never build `owned`; the cgroup the kernel put the process in
  # is the whole answer. Counting `owned` unconditionally crashed every Linux
  # host with `owned: unbound variable` under `set -u`.
  owns() {
    cgroup=$(/bin/cat \"/proc/$1/cgroup\" 2>/dev/null | /usr/bin/sed -n 's/.*\\///p')
    owner_of=''
    case \"$cgroup\" in *.service) owner_of=\"$cgroup\"; return 0 ;; esac
    return 1
  }
fi
owned_count=0
for _pid in $owned; do owned_count=$((owned_count + 1)); done
printf 'STADO_UNOWNED_OWNED\\t%s\\n' \"$owned_count\"
seen=''
for root in \"$@\"; do
  matched=0
  under_count=0
  for pid in $(/usr/bin/pgrep -f \"$root\" 2>/dev/null); do
    matched=$((matched + 1))
    case \" $seen \" in *\" $pid \"*) continue ;; esac
    command=$(/bin/ps -p \"$pid\" -o command= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    if [ -z \"$command\" ]; then continue; fi
    exe=$(/bin/ps -p \"$pid\" -o comm= 2>/dev/null)
    entry=$(printf '%s' \"$command\" | /usr/bin/awk '{ print $2 }')
    # The root has to be what the process EXECUTES, not merely a word on its
    # command line: `pgrep -f` also matches a tail on a log under the root,
    # and a report that names those teaches operators to ignore it. An
    # interpreter is accepted on its entry point, which is the shape a
    # release tree runs under.
    under=no
    case \"$exe\" in \"$root\"*) under=yes ;; esac
    case \"$entry\" in \"$root\"*) under=yes ;; esac
    if [ \"$under\" = no ]; then continue; fi
    under_count=$((under_count + 1))
    if owns \"$pid\"; then
      # The verdict and its evidence, for every candidate. An operator reading
      # \"owned\" needs the pid in the ancestry that launchd actually claimed:
      # a chain that ends on a thousand-entry set is how 26 stado processes on
      # one host were all judged owned and none reported.
      printf 'STADO_UNOWNED_JUDGED\\t%s\\t%s\\t%s\\n' \"$pid\" 'owned' \"$owner_of\"
      continue
    fi
    printf 'STADO_UNOWNED_JUDGED\\t%s\\t%s\\t%s\\n' \"$pid\" 'unowned' '-'
    seen=\"$seen $pid\"
    started=$(/bin/ps -p \"$pid\" -o lstart= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    printf 'STADO_UNOWNED\\t%s\\t%s\\t%s\\n' \"$pid\" \"$started\" \"$command\"
  done
  # What this root actually searched, printed whether or not it found anything.
  # Without it an empty report is indistinguishable from a root that expanded
  # to a path no process could ever run out of, and the empty table was read as
  # \"no unowned processes\" for as long as this command has existed.
  printf 'STADO_UNOWNED_ROOT\\t%s\\t%s\\t%s\\n' \"$root\" \"$matched\" \"$under_count\"
done
";

/// `service logs`: tail the unit's own logs. On launchd the log paths come
/// from the unit file itself, so an adopted unit keeps its chosen
/// destinations. A unit without a StandardOutPath falls back to the
/// account's owner-only Stado log path.
///
/// stderr is a second file under launchd, not a stream the stdout tail
/// already carries, so it gets its own delimited section: `STADO_ERR` names
/// the path and streams its tail, or carries the reason there is nothing to
/// show. It is never silently omitted — a unit that died answering nothing
/// on stdout kept its reason in stderr, and "no section" used to be
/// indistinguishable from "nothing written". The two tails share the
/// --lines budget, half each. On Linux the journal already merges the
/// streams, so that branch stays one section.
const LOGS_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  log=''
  err_log=''
  if [ -f \"$unit_path\" ]; then
    log=$(/usr/bin/plutil -extract StandardOutPath raw -o - \"$unit_path\" 2>/dev/null)
    err_log=$(/usr/bin/plutil -extract StandardErrorPath raw -o - \"$unit_path\" 2>/dev/null)
  fi
  if [ -z \"$log\" ]; then log=\"$HOME/.stado/logs/$unit.log\"; fi
  if [ -f \"$log\" ]; then
    printf 'STADO_LOG\\t%s\\n' \"$log\"
    /usr/bin/tail -c @MAX_BYTES@ \"$log\" | /usr/bin/tail -n @OUT_LINES@
  else
    say 'missing_log' \"$log\"
  fi
  if [ -z \"$err_log\" ]; then
    printf 'STADO_ERR\\t%s\\n' 'absent in plist'
  elif [ -s \"$err_log\" ]; then
    printf 'STADO_ERR\\t%s\\n' \"$err_log\"
    /usr/bin/tail -c @MAX_BYTES@ \"$err_log\" | /usr/bin/tail -n @ERR_LINES@
  else
    printf 'STADO_ERR\\t%s\\n' \"$err_log (empty)\"
  fi
else
  if [ \"$scope\" = \"system\" ]; then
    printf 'STADO_LOG\\tjournalctl -u %s\\n' \"$unit\"
    stado_root /usr/bin/journalctl -u \"$unit\" -n @LINES@ --no-pager 2>&1
  else
    printf 'STADO_LOG\\tjournalctl --user -u %s\\n' \"$unit\"
    runtime=\"/run/user/$service_uid\"
    if [ \"$service_uid\" = \"$uid\" ]; then
      /usr/bin/env \
        XDG_RUNTIME_DIR=\"$runtime\" \
        DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
        /usr/bin/journalctl --user -u \"$unit\" -n @LINES@ --no-pager 2>&1
    else
      \"$sudo_bin\" -n -u \"$service_user\" /usr/bin/env \
        XDG_RUNTIME_DIR=\"$runtime\" \
        DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
        /usr/bin/journalctl --user -u \"$unit\" -n @LINES@ --no-pager 2>&1
    fi
  fi
fi
";

/// `service env`: fetch the complete unit definition, including systemd drop-ins.
/// Parsing on this side keeps the remote program fixed and narrow, and
/// keeps redaction in one place instead of trusting a shell pipeline to
/// have caught every credential-shaped key.
const UNIT_FILE_BODY: &str = "if [ \"$os\" = Linux ]; then
  if ! content=$(stado_systemctl cat --no-pager \"$unit\" 2>&1); then
    say 'unit_definition_unavailable' \"$content\"
    exit 1
  fi
  printf 'STADO_UNITFILE\\t%s\\n%s\\n' \"$unit_path\" \"$content\"
elif [ -f \"$unit_path\" ]; then
  printf 'STADO_UNITFILE\\t%s\\n' \"$unit_path\"
  /bin/cat \"$unit_path\"
else
  say 'missing_unit_file' \"$unit_path\"
fi
";

/// Replace one assignment in an owner-only runtime environment file.
///
/// The secret rides inside the SSH request body as base64, never argv. The
/// remote shell decodes a complete shell-quoted assignment, removes prior
/// assignments of the same variable, and atomically renames a mode-600 file.
/// Existing unrelated variables stay on the host and never cross back to the
/// operator.
const SECRET_SYNC_BODY: &str = "fail_sync() {
  say 'secret_sync_failed' \"$1\"
  exit 0
}
if [ \"$os\" = \"Darwin\" ]; then decode_flag=-D; else decode_flag=--decode; fi
env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_sync 'invalid environment path payload'
case \"$env_path\" in
  \\$HOME/*) env_path=\"$HOME/${env_path#\\$HOME/}\" ;;
  /*) ;;
  *) fail_sync 'environment path is not rooted' ;;
esac
variable=@VARIABLE@
stado_sync_item=@ITEM@
stado_sync_field=@FIELD@
if [ -n \"$stado_sync_item\" ]; then
  value=$(\"$HOME/.stado/bin/stado\" credentials get \"$stado_sync_item\" --field \"$stado_sync_field\" 2>/dev/null) || fail_sync 'bearer unavailable on this host'
  [ -n \"$value\" ] || fail_sync 'bearer field is empty'
  export variable
  assignment=$(/usr/bin/env STADO_SYNC_VALUE=\"$value\" /usr/bin/python3 -c 'import os, shlex; print(os.environ[\"variable\"] + \"=\" + shlex.quote(os.environ[\"STADO_SYNC_VALUE\"]))' 2>/dev/null) || fail_sync 'cannot render the assignment'
else
  assignment=$(printf '%s' '@ASSIGNMENT_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_sync 'invalid assignment payload'
fi
parent=$(/usr/bin/dirname \"$env_path\") || fail_sync 'environment parent unavailable'
/bin/mkdir -p \"$parent\" || fail_sync 'cannot create environment parent'
tmp=\"$env_path.stado-secret-sync.$$\"
trap '/bin/rm -f \"$tmp\"' EXIT HUP INT TERM
if [ -f \"$env_path\" ]; then
  /usr/bin/awk -v key=\"$variable\" '
    $0 ~ \"^[[:space:]]*(export[[:space:]]+)?\" key \"=\" { next }
    { print }
  ' \"$env_path\" > \"$tmp\" || fail_sync 'cannot filter environment file'
else
  : > \"$tmp\" || fail_sync 'cannot create environment file'
fi
printf '%s\\n' \"$assignment\" >> \"$tmp\" || fail_sync 'cannot append assignment'
/bin/chmod 600 \"$tmp\" || fail_sync 'cannot protect environment file'
/bin/mv -f \"$tmp\" \"$env_path\" || fail_sync 'cannot install environment file'
trap - EXIT HUP INT TERM
say 'secret_synced' \"$variable $env_path\"
";

/// Authenticate one read-only loopback request from the managed host.
///
/// The bearer is staged in an owner-only curl header file, never argv. Both
/// the header and response body are removed before the marker is emitted.
const AUTH_CHECK_BODY: &str = "stado_check_item=@ITEM@
stado_check_field=@FIELD@
stado_check_var=@VARIABLE@
stado_check_env_b64=@ENV_PATH_B64@
stado_check_consumer=@CONSUMER@
stado_check_token_file=@TOKEN_FILE@
fail_check() {
  say 'auth_check_failed' \"$1\"
  exit 0
}
if [ \"$os\" = \"Darwin\" ]; then decode_flag=-D; else decode_flag=--decode; fi
probe_url=$(printf '%s' '@PROBE_URL_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_check 'invalid probe URL payload'
if [ -n \"$stado_check_consumer\" ]; then
  export WC_SKARBIEC_CONSUMER=\"$stado_check_consumer\"
fi
if [ -n \"$stado_check_token_file\" ]; then
  export WC_SKARBIEC_TOKEN_FILE=\"$stado_check_token_file\"
fi
probe_dir=\"$HOME/.stado/auth-check\"
/bin/mkdir -p \"$probe_dir\" || fail_check 'cannot create probe directory'
/bin/chmod 700 \"$probe_dir\" || fail_check 'cannot protect probe directory'
resolve_err=\"\"
resolved=\"\"
if [ -n \"$stado_check_item\" ]; then
  probe_log=\"$probe_dir/resolve.log\"
  : > \"$probe_log\"
  resolved=$(\"$HOME/.stado/bin/stado\" secrets get \"$stado_check_item\" --field \"$stado_check_field\" 2>\"$probe_log\")
  src=secrets-get
  # Source 2/3: legacy credential-store and direct Skarbiec reads.
  if [ -z \"$resolved\" ]; then
    resolved=$(\"$HOME/.stado/bin/stado\" credentials get \"$stado_check_item\" --field \"$stado_check_field\" 2>>\"$probe_log\") && src=credentials-get
  fi
  if [ -z \"$resolved\" ]; then
    resolved=$(\"$HOME/.stado/bin/skarbiec\" get \"$stado_check_item\" --field \"$stado_check_field\" 2>>\"$probe_log\") && src=skarbiec-get
  fi
  [ -n \"$resolved\" ] || { resolve_err=$(src=$src; /usr/bin/tail -c 300 \"$probe_log\" 2>/dev/null); fail_check \"bearer unavailable via $src${resolve_err:+: $resolve_err}\"; }
elif [ -n \"$stado_check_var\" ]; then
  check_env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_check 'invalid environment path payload'
  case \"$check_env_path\" in
    \\$HOME/*) check_env_path=\"$HOME/${check_env_path#\\$HOME/}\" ;;
    /*) ;;
    *) fail_check 'environment path is not rooted' ;;
  esac
  [ -f \"$check_env_path\" ] || fail_check 'runtime environment file is absent'
  resolved=$(/usr/bin/awk -F= -v key=\"$stado_check_var\" '$1 == key { v=substr($0, length($1)+2); gsub(/^[\"]+|[\"]+$/, \"\", v); print v }' \"$check_env_path\")
  [ -n \"$resolved\" ] || fail_check 'environment variable is empty or absent'
else
  resolved=$(printf '%s' '@TOKEN_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_check 'invalid token payload'
fi
token=\"$resolved\"
[ -n \"$token\" ] || fail_check 'bearer field is empty'
post_empty=@POST_EMPTY@
expected_status=@EXPECTED_STATUS@
probe_dir=\"$HOME/.stado/auth-check\"
/bin/mkdir -p \"$probe_dir\" || fail_check 'cannot create probe directory'
/bin/chmod 700 \"$probe_dir\" || fail_check 'cannot protect probe directory'
header=\"$probe_dir/header.$$\"
response=\"$probe_dir/response.$$\"
error_file=\"$probe_dir/error.$$\"
trap '/bin/rm -f \"$header\" \"$response\" \"$error_file\"' EXIT HUP INT TERM
printf 'Authorization: Bearer %s\\n' \"$token\" > \"$header\" || fail_check 'cannot stage authorization header'
unset token
/bin/chmod 600 \"$header\" || fail_check 'cannot protect authorization header'
if [ \"$post_empty\" = yes ]; then
  status=$(/usr/bin/curl --silent --show-error --max-time 15 --output \"$response\" --write-out '%{http_code}' --request POST --header 'Content-Type: application/json' --header \"@$header\" --data '{}' \"$probe_url\" 2>\"$error_file\")
  rc=$?
else
  status=$(/usr/bin/curl --silent --show-error --max-time 15 --output \"$response\" --write-out '%{http_code}' --header \"@$header\" \"$probe_url\" 2>\"$error_file\")
  rc=$?
fi
/bin/rm -f \"$header\" \"$response\" \"$error_file\"
trap - EXIT HUP INT TERM
if [ \"$rc\" -ne 0 ]; then
  say 'auth_unreachable' \"curl exit $rc\"
elif [ -n \"$expected_status\" ] && [ \"$status\" = \"$expected_status\" ]; then
  say 'auth_ok' \"HTTP $status\"
elif [ -z \"$expected_status\" ] && [ \"$status\" -ge 200 ] 2>/dev/null && [ \"$status\" -lt 300 ] 2>/dev/null; then
  say 'auth_ok' \"HTTP $status\"
elif [ \"$status\" = 401 ] || [ \"$status\" = 403 ]; then
  say 'auth_rejected' \"HTTP $status\"
else
  say 'auth_failed' \"HTTP $status\"
fi
";

/// Stop the process currently owning a checked loopback port.
///
/// This is deliberately Darwin-only and separate from ordinary restart:
/// launchd cannot replace an unmanaged fallback process that still owns the
/// service port.
const LISTENER_RESET_BODY: &str = "if [ \"$os\" != \"Darwin\" ]; then
  say 'listener_reset_unsupported' \"$os\"
  exit 0
fi
port=@PORT@
pids=$(/usr/sbin/lsof -nP -tiTCP:\"$port\" -sTCP:LISTEN 2>/dev/null)
if [ -z \"$pids\" ]; then
  say 'listener_absent' \"$port\"
  exit 0
fi
listener_detail=\"$port\"
for pid in $pids; do
  case \"$pid\" in *[!0-9]*) say 'listener_reset_failed' 'invalid pid'; exit 0 ;; esac
  owner=$(/bin/ps -p \"$pid\" -o ppid=,comm= 2>/dev/null | /usr/bin/tr '\t\r\n' ' ')
  listener_detail=\"$listener_detail pid=$pid $owner\"
  /bin/kill -TERM \"$pid\" >/dev/null 2>&1 || true
done
/bin/sleep 1
for pid in $pids; do
  if /bin/kill -0 \"$pid\" >/dev/null 2>&1; then
    /bin/kill -KILL \"$pid\" >/dev/null 2>&1 || true
  fi
done
say 'listener_stopped' \"$listener_detail\"
";

/// The shared prelude with this unit spliced in: the vocabulary (`$unit`,
/// `$domain`, `$domain_status`, `$domain_reason`, `$launch`,
/// `stado_systemctl`, `say`, and the three `stado_unit_*` reads) every body and
/// every postcondition probe reads the host through.
///
/// [`DOMAIN_RESOLVER`] and [`UNIT_STATE`] are spliced here and nowhere else,
/// so no body can answer "which domain is this unit in" or "which processes
/// are this unit" for itself. Both questions used to be answered inline, per
/// body, and the answers disagreed.
///
/// `no_domain` is what the prelude does when a Darwin host has no per-login
/// launchd domain at all. It is a parameter and not a fixed refusal because
/// the two answers are genuinely different operations: everything that
/// addresses an installed unit has nothing to act on ([`NO_DOMAIN_REFUSE`]),
/// while [`ensure_service`] installs the daemon spelling into the domain that
/// does exist ([`NO_DOMAIN_SYSTEM`]).
fn prelude_with(
    unit: &str,
    linux_unit: &str,
    path: &str,
    no_domain: &str,
    observed_domain: Option<&str>,
) -> Result<String, DeployError> {
    validate_unit_id(unit)?;
    Ok(REMOTE_PRELUDE
        .replace("@DOMAIN_RESOLVER@", DOMAIN_RESOLVER)
        .replace(
            "@OBSERVED_DOMAIN@",
            &observed_domain.map_or_else(String::new, |domain| {
                format!(
                    "  domain={}\n  domain_status={}\n  domain_reason='the exact loaded owner was observed before this lifecycle action'\n",
                    shlex_quote(domain),
                    if domain.starts_with("gui/") { "graphical" } else { "fallback" },
                )
            }),
        )
        .replace("@UNIT_STATE@", UNIT_STATE)
        .replace("@UNIT@", &shlex_quote(unit))
        .replace("@LINUX_UNIT@", &shlex_quote(linux_unit))
        .replace("@PATH@", &quote_unit_path(path)?)
        .replace("@NO_DOMAIN@", no_domain))
}

fn remote_prelude(unit: &str, linux_unit: &str, path: &str) -> Result<String, DeployError> {
    prelude_with(unit, linux_unit, path, NO_DOMAIN_REFUSE, None)
}

/// Assemble a remote program: the shared prelude with this unit spliced in,
/// then one fixed body.
fn remote_script(
    unit: &str,
    linux_unit: &str,
    path: &str,
    body: &str,
) -> Result<String, DeployError> {
    let prelude = remote_prelude(unit, linux_unit, path)?;
    Ok(format!("{prelude}{body}"))
}

/// The shared prelude with one unit spliced in, ahead of a caller's own body.
///
/// [`super::service_serving`] needs exactly the vocabulary every body here
/// reads the host through — `$unit`, `$unit_path`, `$domain`, `$launch`,
/// `stado_launchd_state` — and must not grow a second copy of it. A second
/// resolver for "which domain is this unit in" is how two commands come to
/// disagree about the same unit, which is the failure [`prelude_with`] exists
/// to prevent.
pub fn serving_script(unit: &str, path: &str, body: &str) -> Result<String, DeployError> {
    remote_script(unit, "", path, body)
}

// ---------------------------------------------------------------------------
// Write side: one command per remote program
// ---------------------------------------------------------------------------

/// Report the argument vector a managed unit runs, exactly as declared.
pub async fn show_service(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, SHOW_BODY)?;
    run_remote(target, script, runner).await
}

// ---------------------------------------------------------------------------
// The one repair a system LaunchDaemon has that needs no privilege
// ---------------------------------------------------------------------------

/// `KeepAlive` is `<true/>`: launchd recreates the process whenever it ends,
/// for any reason. This is the only spelling that authorizes ending the
/// process, because it is the only one under which the answer to "will
/// something put it back" is yes without reading further keys.
pub const KEEP_ALIVE_ALWAYS: &str = "true";
/// `KeepAlive` is a dict (`SuccessfulExit`, `Crashed`, `PathState`, ...).
/// launchd may or may not respawn after a signal depending on those keys,
/// and guessing which is not a thing to do to a control plane.
pub const KEEP_ALIVE_CONDITIONAL: &str = "conditional";
/// The unit declares no `KeepAlive` at all.
pub const KEEP_ALIVE_ABSENT: &str = "absent";
/// The plist could not be read, so nothing about respawning is known.
pub const KEEP_ALIVE_UNREADABLE: &str = "unreadable";

/// What one system LaunchDaemon looks like from the approved unprivileged
/// login: its respawn declaration, this login's account, and which of the
/// pids running its program that account owns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemDaemon {
    /// [`KEEP_ALIVE_ALWAYS`], [`KEEP_ALIVE_CONDITIONAL`],
    /// [`KEEP_ALIVE_ABSENT`], [`KEEP_ALIVE_UNREADABLE`], or the literal
    /// scalar the plist carries (`false` is the one that matters).
    pub keep_alive: String,
    /// The account the approved channel logs in as.
    pub login_user: String,
    /// Pids running exactly the argv this unit declares, that
    /// [`Self::login_user`] owns and can therefore signal without privilege.
    pub owned_pids: Vec<String>,
    /// Pids running it that some other account owns.
    pub foreign_pids: Vec<String>,
    /// The whole argument vector the unit declares, single-spaced.
    ///
    /// The argv and not the program: every Stado service on control-host
    /// runs `/Users/charles/.stado/bin/stado`, so the program is the fleet and
    /// the argv is the unit. A restart that resolved its pids by program TERMed
    /// eight processes there on 2026-08-19 and reported one unit restarted.
    pub argv: String,
}

impl SystemDaemon {
    /// True when launchd will unconditionally put a new process in place of
    /// one that ends.
    pub fn respawns(&self) -> bool {
        self.keep_alive == KEEP_ALIVE_ALWAYS
    }

    /// True when this login can perform the whole restart on its own: the
    /// process is one it owns, and launchd is keeping the job alive.
    pub fn restartable_unprivileged(&self) -> bool {
        self.respawns() && !self.owned_pids.is_empty()
    }

    /// Why this daemon cannot be restarted from here, in the operator's
    /// words. Only reached when [`Self::restartable_unprivileged`] is false,
    /// and it always names the privileged command that does work.
    fn refusal(&self, service: &ManagedService) -> String {
        let reason = if !self.respawns() {
            match self.keep_alive.as_str() {
                KEEP_ALIVE_ABSENT => "the unit declares no KeepAlive, so ending its process would \
                                      leave nothing to start another one and this host would go \
                                      from degraded to down"
                    .to_string(),
                KEEP_ALIVE_CONDITIONAL => "the unit declares a conditional KeepAlive, so whether \
                                           launchd respawns it after a signal depends on keys \
                                           this channel must not guess at"
                    .to_string(),
                KEEP_ALIVE_UNREADABLE => "the unit's plist could not be read, so whether anything \
                                          would start another process is unknown"
                    .to_string(),
                other => format!(
                    "the unit declares KeepAlive {other}, so launchd will not start another \
                     process when this one ends"
                ),
            }
        } else if !self.foreign_pids.is_empty() {
            format!(
                "its process runs as another account (pid(s) {}), not as the approved user {}, so \
                 this channel cannot signal it",
                self.foreign_pids.join(" "),
                self.login_user
            )
        } else {
            format!(
                "nothing on the host is running {}, so there is no process to end and launchd is \
                 not holding the job up",
                if self.argv.is_empty() {
                    "the unit's declared argv"
                } else {
                    &self.argv
                }
            )
        };
        format!(
            "{} on {} is a system LaunchDaemon at {}; the approved channel is unprivileged and \
             cannot bootstrap it, and {reason}. Restarting it needs one privileged command on the \
             host: sudo launchctl kickstart -k system/{}",
            service.unit_id(),
            service.host,
            service.path,
            service.unit_id()
        )
    }
}

/// The `STADO_DAEMON` marker. Absent for every path that never reached the
/// probe (a missing unit file, an unsupported OS), which is why it is an
/// [`Option`].
fn parse_daemon(stdout: &str) -> Option<SystemDaemon> {
    let words = |field: &str| -> Vec<String> {
        field
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<String>>()
    };
    for line in stdout.lines() {
        if let ["STADO_DAEMON", keep_alive, login_user, owned, foreign, argv] =
            host_channel::marker_fields(line).as_slice()
        {
            return Some(SystemDaemon {
                keep_alive: (*keep_alive).to_string(),
                login_user: (*login_user).to_string(),
                owned_pids: words(owned),
                foreign_pids: words(foreign),
                argv: (*argv).to_string(),
            });
        }
    }
    None
}

/// The pids the terminate program may signal, as one shell word list.
///
/// Every value here was reported by the host's own `pgrep` moments ago, but
/// it still travels back over the channel as data, and a signal list is the
/// last place to trust a round trip. Digits and single spaces only; anything
/// else is refused rather than quoted, because the useful failure is "the
/// host said something this operation does not understand", never a
/// creatively escaped `kill` argument.
fn validate_pid_list(pids: &[String]) -> Result<String, DeployError> {
    for pid in pids {
        if pid.is_empty() || !pid.chars().all(|character| character.is_ascii_digit()) {
            return Err(DeployError(format!(
                "the host reported {} as a process id of this unit, which is not a process id",
                py_str_repr(pid)
            )));
        }
    }
    Ok(pids.join(" "))
}

/// Read one system LaunchDaemon's respawn declaration and process ownership.
///
/// Read-only: it starts nothing, stops nothing and signals nothing, so it is
/// safe against a live production host.
pub async fn inspect_system_daemon(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<(RemoteReport, Option<SystemDaemon>), DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, DAEMON_PROBE_BODY)?;
    let report = run_remote(target, script, runner).await?;
    let daemon = parse_daemon(&report.stdout);
    Ok((report, daemon))
}

/// `service restart` on one host, with the end state it intends checked on
/// the host before the connection closes. A restart whose own steps report
/// success while the unit ends up unloaded is reported as the failure it
/// is: see [`RemoteReport::succeeded`].
///
/// A unit in the system domain takes a different route, because the approved
/// channel is unprivileged and `launchctl bootstrap system` is not available
/// to it. It is not, however, unrecoverable: every daemon this fleet installs
/// carries `UserName`, so the process runs as the approved user even though
/// the job is root's, and it carries `KeepAlive` `<true/>`, so launchd puts a
/// new process in place of one that ends. Ending the process from the account
/// that owns it is therefore the same sequence `launchctl kickstart -k`
/// performs — the job is never unloaded, and there is no window in which it
/// does not exist.
///
/// Both gates are read from the host first ([`inspect_system_daemon`]) and
/// neither is assumed. Without them the command refuses and names the one
/// privileged command that works, because ending a process nothing will
/// respawn is how a degraded control plane becomes a dead one. That refusal
/// used to be the only answer here, and it sent the operator to
/// `stado host recover`, which does not re-bootstrap a system daemon either:
/// on 2026-08-19 the object API answered 503 to the whole fleet for an
/// afternoon with no product path back.
pub async fn restart_service(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    restart_service_with_password(target, service, None, runner).await
}

pub async fn restart_service_with_password(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if UnitDomain::from_path(&service.path).requires_privileged_bootstrap() {
        return restart_system_daemon(target, service, sudo_password, runner).await;
    }
    restart_non_system_service(target, service, None, false, runner).await
}

async fn restart_non_system_service(
    target: &ComputeTarget,
    service: &ManagedService,
    observed_domain: Option<&str>,
    reload_unit: bool,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = format!(
        "stado_reload_unit={}\n{}",
        u8::from(reload_unit),
        RESTART_BODY.replace("@DISOWNED_SWEEP@", DISOWNED_SWEEP)
    );
    let prelude = prelude_with(
        service.unit_id(),
        "",
        &service.path,
        NO_DOMAIN_REFUSE,
        observed_domain,
    )?;
    let mut report = run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RUNNING_DESCRIBE, RUNNING_PROBE),
        runner,
    )
    .await?;
    report.name_unloaded(service.unit_id(), "restart");
    Ok(report)
}
/// Reload one system LaunchDaemon definition and wait for its owned process.
///
/// Unlike `kickstart`, this performs `bootout` and `bootstrap`, so launchd
/// reads changed ProgramArguments from the plist before the service is checked.
pub async fn reload_service_with_password(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if !matches!(UnitDomain::from_path(&service.path), UnitDomain::System) {
        return Err(DeployError(
            "unit reload is supported only for a system LaunchDaemon".to_string(),
        ));
    }
    privileged_restart_system_daemon(
        target,
        service,
        sudo_password.unwrap_or_default(),
        true,
        runner,
    )
    .await
}

/// The system-domain half of [`restart_service`]: probe, then either end the
/// owned process and let launchd recreate it, or refuse with the privileged
/// command named.
async fn restart_system_daemon(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let (probe, daemon) = inspect_system_daemon(target, service, runner).await?;
    let Some(daemon) = daemon else {
        // No marker: the probe never got as far as reading the unit. Its own
        // report already carries why (a missing unit file, a refused key),
        // and that is a better answer than a refusal composed here.
        return Ok(probe);
    };
    let cached = super::service_label_print::print_label(
        target,
        service.unit_id(),
        BootoutScope::System,
        runner,
    )
    .await?;
    if cached.loaded() {
        let cached_argv = cached.runs().ok_or_else(|| {
            DeployError(format!(
                "{} has no readable cached launchd argument vector",
                service.unit_id()
            ))
        })?;
        if cached_argv != daemon.argv {
            return privileged_restart_system_daemon(
                target,
                service,
                sudo_password.unwrap_or_default(),
                true,
                runner,
            )
            .await;
        }
    }
    if !daemon.restartable_unprivileged() {
        if let Some(password) = sudo_password {
            return privileged_restart_system_daemon(target, service, password, false, runner)
                .await;
        }
        return privileged_restart_system_daemon(target, service, "", false, runner)
            .await
            .map_err(|error| {
                DeployError(format!(
                    "{}; passwordless privileged restart also failed: {error}",
                    daemon.refusal(service)
                ))
            });
    }
    let body = DAEMON_TERM_BODY
        .replace("@ARGV@", &shlex_quote(&daemon.argv))
        .replace(
            "@PIDS@",
            &shlex_quote(&validate_pid_list(&daemon.owned_pids)?),
        );
    let prelude = remote_prelude(service.unit_id(), "", &service.path)?;
    let mut report = run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RESPAWNED_DESCRIBE, RESPAWNED_PROBE),
        runner,
    )
    .await?;
    if report.succeeded("restarted") {
        // The host's own detail says what happened, in the 160 characters one
        // marker field allows. Why that counts as a restart is a fixed
        // sentence about launchd, not a fact about this host, so it is stated
        // here instead of eating the framing budget on every pass. Without it
        // an operator reading `restarted` beside a `kill` has to take the
        // equivalence on trust.
        report.detail = format!(
            "{} — that is what `launchctl kickstart -k` does to a KeepAlive job, minus the \
             privilege it needs: the process is replaced and the job is never unloaded",
            report.detail
        );
        return Ok(report);
    }
    if let Some(password) = sudo_password {
        return privileged_restart_system_daemon(target, service, password, false, runner).await;
    }
    Ok(report)
}

/// Restart or reload one system LaunchDaemon through the host account credential.
///
/// The password travels only on the Stado host channel's stdin to `sudo -S`;
/// neither the password nor a shell program containing it is present in argv
/// or command output.
async fn privileged_restart_system_daemon(
    target: &ComputeTarget,
    service: &ManagedService,
    password: &str,
    reload_unit: bool,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_unit_id(service.unit_id())?;
    let qualified = format!("system/{}", service.unit_id());
    let recovery = format!("system/{}-recovery", service.unit_id());
    if reload_unit {
        let lint =
            host_channel::run_program(target, &["/usr/bin/plutil", "-lint", &service.path], runner)
                .await?;
        if !lint.ok() {
            return Err(DeployError(format!(
                "refusing to reload invalid LaunchDaemon plist {} on {}: {}",
                service.path,
                target.name,
                host_channel::last_error_line(&lint, "plutil returned no detail")
            )));
        }
    }
    let recovery_stop = host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/sudo",
            "-S",
            "-p",
            "",
            "/bin/launchctl",
            "bootout",
            &recovery,
        ],
        &format!("{password}\n"),
        runner,
    )
    .await?;
    if !recovery_stop.ok() {
        let detail =
            host_channel::last_error_line(&recovery_stop, "sudo or launchctl returned no detail");
        if !detail.contains("Could not find specified service")
            && !detail.contains("No such process")
        {
            return Err(DeployError(format!(
                "privileged recovery stop failed on {} with exit {}: {}",
                target.name, recovery_stop.code, detail
            )));
        }
    }
    let mut output = if reload_unit {
        let bootout = host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "bootout",
                &qualified,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?;
        if !bootout.ok() {
            let detail =
                host_channel::last_error_line(&bootout, "sudo or launchctl returned no detail");
            if !detail.contains("Could not find specified service")
                && !detail.contains("No such process")
            {
                return Err(DeployError(format!(
                    "privileged launchd bootout failed on {} with exit {}: {}",
                    target.name, bootout.code, detail
                )));
            }
        }
        let mut unloaded = false;
        for _ in 0..15 {
            let print = host_channel::run_program_with_stdin(
                target,
                &[
                    "/usr/bin/sudo",
                    "-S",
                    "-p",
                    "",
                    "/bin/launchctl",
                    "print",
                    &qualified,
                ],
                &format!("{password}\n"),
                runner,
            )
            .await?;
            if !print.ok() {
                unloaded = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if !unloaded {
            return Err(DeployError(format!(
                "privileged launchd bootout on {} returned, but {} remained loaded after 15s",
                target.name, qualified
            )));
        }
        let enable = host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "enable",
                &qualified,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?;
        if !enable.ok() {
            return Err(DeployError(format!(
                "privileged launchd enable failed on {} with exit {}: {}",
                target.name,
                enable.code,
                host_channel::last_error_line(&enable, "sudo or launchctl returned no detail")
            )));
        }
        host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "bootstrap",
                "system",
                &service.path,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?
    } else {
        host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "kickstart",
                "-k",
                &qualified,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?
    };
    if !output.ok() && !reload_unit {
        let bootstrap = host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "bootstrap",
                "system",
                &service.path,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?;
        if bootstrap.ok() {
            output = host_channel::run_program_with_stdin(
                target,
                &[
                    "/usr/bin/sudo",
                    "-S",
                    "-p",
                    "",
                    "/bin/launchctl",
                    "kickstart",
                    "-k",
                    &qualified,
                ],
                &format!("{password}\n"),
                runner,
            )
            .await?;
        }
    }
    if !output.ok() {
        return Err(DeployError(format!(
            "privileged launchd restart failed on {} with exit {}: {}",
            target.name,
            output.code,
            host_channel::last_error_line(&output, "sudo or launchctl returned no detail")
        )));
    }

    for _ in 0..15 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let (_, daemon) = inspect_system_daemon(target, service, runner).await?;
        if let Some(daemon) = daemon.filter(|daemon| !daemon.owned_pids.is_empty()) {
            return Ok(RemoteReport {
                os: "Darwin".to_string(),
                domain: "system".to_string(),
                domain_status: DOMAIN_STATUS_SYSTEM.to_string(),
                domain_reason: "the unit file is a system LaunchDaemon".to_string(),
                unit: service.unit_id().to_string(),
                path: service.path.clone(),
                status: "restarted".to_string(),
                detail: format!(
                    "launchctl {} the system daemon with pid(s) {}",
                    if reload_unit { "reloaded" } else { "restarted" },
                    daemon.owned_pids.join(" ")
                ),
                postcondition: RUNNING_DESCRIBE.to_string(),
                postcondition_state: host_channel::POSTCONDITION_MET.to_string(),
                postcondition_detail: "launchd reports a process for the unit".to_string(),
                ..RemoteReport::default()
            });
        }
    }
    Err(DeployError(format!(
        "{} accepted the privileged {} but no process appeared for {} in 15 seconds",
        target.name,
        if reload_unit { "reload" } else { "kickstart" },
        service.unit_id()
    )))
}

/// Atomically replace one runtime secret assignment for a managed service.
pub async fn sync_service_secret(
    target: &ComputeTarget,
    service: &ManagedService,
    env_path: &str,
    variable: &str,
    secret: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_home_rooted_file(env_path, "environment file")?;
    validate_env_variable(variable)?;
    validate_secret_value(secret)?;

    let assignment = format!("{variable}={}\n", shlex_quote(secret));
    let body = SECRET_SYNC_BODY
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@VARIABLE@", &shlex_quote(variable))
        .replace("@ASSIGNMENT_B64@", &STANDARD.encode(assignment.as_bytes()));
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// Verify that one bearer reaches an authenticated loopback endpoint.
pub async fn check_service_bearer(
    target: &ComputeTarget,
    service: &ManagedService,
    probe_url: &str,
    token: &str,
    post_empty_json: bool,
    expected_status: Option<u16>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_loopback_probe_url(probe_url)?;
    validate_secret_value(token)?;
    let body = AUTH_CHECK_BODY
        .replace("@PROBE_URL_B64@", &STANDARD.encode(probe_url.as_bytes()))
        .replace("@TOKEN_B64@", &STANDARD.encode(token.as_bytes()))
        .replace("@POST_EMPTY@", if post_empty_json { "yes" } else { "no" })
        .replace(
            "@EXPECTED_STATUS@",
            &shlex_quote(
                &expected_status
                    .map(|status| status.to_string())
                    .unwrap_or_default(),
            ),
        );
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// [`sync_service_secret`] with the bearer resolved on the host: the item is
/// read there by the host's own Stado identity, so the value never travels
/// on this channel and the operator's consumer needs no grant for it.
pub async fn sync_service_item_secret(
    target: &ComputeTarget,
    service: &ManagedService,
    env_path: &str,
    variable: &str,
    item: &str,
    field: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_home_rooted_file(env_path, "environment file")?;
    validate_env_variable(variable)?;
    validate_vault_reference(item, field)?;

    let body = SECRET_SYNC_BODY
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@VARIABLE@", &shlex_quote(variable))
        .replace("@ITEM@", &shlex_quote(item))
        .replace("@FIELD@", &shlex_quote(field));
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// [`check_service_bearer`] with the bearer resolved on the host from one
/// Skarbiec item field. The probe reports only its HTTP outcome; the bearer
/// itself never leaves the host.
// Each argument is one independently validated piece of the fixed remote
// authentication probe; bundling them would only obscure the call contract.
#[allow(clippy::too_many_arguments)]
pub async fn check_service_item_bearer(
    target: &ComputeTarget,
    service: &ManagedService,
    probe_url: &str,
    item: &str,
    field: &str,
    consumer: Option<&str>,
    token_file: Option<&str>,
    post_empty_json: bool,
    expected_status: Option<u16>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_loopback_probe_url(probe_url)?;
    validate_vault_reference(item, field)?;
    let body = AUTH_CHECK_BODY
        .replace("@PROBE_URL_B64@", &STANDARD.encode(probe_url.as_bytes()))
        .replace("@ITEM@", &shlex_quote(item))
        .replace("@CONSUMER@", &shlex_quote(consumer.unwrap_or_default()))
        .replace("@TOKEN_FILE@", &shlex_quote(token_file.unwrap_or_default()))
        .replace("@FIELD@", &shlex_quote(field))
        .replace("@TOKEN_B64@", "")
        .replace("@POST_EMPTY@", if post_empty_json { "yes" } else { "no" })
        .replace(
            "@EXPECTED_STATUS@",
            &shlex_quote(
                &expected_status
                    .map(|status| status.to_string())
                    .unwrap_or_default(),
            ),
        );
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// [`check_service_item_bearer`] reading the bearer from the unit's own
/// runtime environment file -- the exact assignment the running process was
/// started with. This is the zero-grant diagnostic path: no Skarbiec read is
/// involved on either side.
// This mirrors the item-backed probe while selecting an environment bearer;
// the explicit arguments keep the two security boundaries visible.
#[allow(clippy::too_many_arguments)]
pub async fn check_service_env_bearer(
    target: &ComputeTarget,
    service: &ManagedService,
    probe_url: &str,
    env_path: &str,
    variable: &str,
    post_empty_json: bool,
    expected_status: Option<u16>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_loopback_probe_url(probe_url)?;
    validate_home_rooted_file(env_path, "environment file")?;
    validate_env_variable(variable)?;
    let body = AUTH_CHECK_BODY
        .replace("@PROBE_URL_B64@", &STANDARD.encode(probe_url.as_bytes()))
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@VARIABLE@", &shlex_quote(variable))
        .replace("@ITEM@", "")
        .replace("@FIELD@", "")
        .replace("@TOKEN_B64@", "")
        .replace("@POST_EMPTY@", if post_empty_json { "yes" } else { "no" })
        .replace(
            "@EXPECTED_STATUS@",
            &shlex_quote(
                &expected_status
                    .map(|status| status.to_string())
                    .unwrap_or_default(),
            ),
        );
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// Item and field names travel verbatim into the fixed remote program, so
/// they carry the same charset contract a launchd label does: nothing that
/// could close the surrounding quotes or open a substitution.
fn validate_vault_reference(item: &str, field: &str) -> Result<(), DeployError> {
    let acceptable = |value: &str| {
        !value.is_empty()
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
    };
    if acceptable(item) && acceptable(field) {
        Ok(())
    } else {
        Err(DeployError(
            "Skarbiec item and field must be non-empty and use only letters, digits, '-', '_' and '.'"
                .to_string(),
        ))
    }
}

/// Stop an explicitly declared per-login recovery label before taking over its
/// listener. Recovery labels are not derived from the primary unit: older
/// deployments used service names while the managed unit used launchd labels.
pub async fn stop_recovery_unit(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_unit_id(unit)?;
    let uid_output = host_channel::run_program(target, &["/usr/bin/id", "-u"], runner).await?;
    if !uid_output.ok() {
        return Err(DeployError(format!(
            "{}: cannot resolve the GUI user's uid: {}",
            target.name,
            host_channel::last_error_line(&uid_output, "id returned no detail")
        )));
    }
    let uid = uid_output.stdout.trim();
    if uid.is_empty() || !uid.chars().all(|character| character.is_ascii_digit()) {
        return Err(DeployError(format!(
            "{}: id returned an invalid uid: {}",
            target.name, uid
        )));
    }
    for domain in ["gui", "user"] {
        let qualified = format!("{domain}/{uid}/{unit}");
        let output =
            host_channel::run_program(target, &["/bin/launchctl", "bootout", &qualified], runner)
                .await?;
        if !output.ok() {
            let detail = host_channel::last_error_line(&output, "launchctl returned no detail");
            if !detail.contains("Could not find specified service")
                && !detail.contains("No such process")
            {
                return Err(DeployError(format!(
                    "{}: cannot stop recovery label {qualified}: {detail}",
                    target.name
                )));
            }
        }
    }
    Ok(RemoteReport {
        os: "Darwin".to_string(),
        unit: unit.to_string(),
        status: "stopped".to_string(),
        detail: "recovery label removed from gui and user launchd domains".to_string(),
        postcondition: STOPPED_DESCRIBE.to_string(),
        postcondition_state: host_channel::POSTCONDITION_MET.to_string(),
        postcondition_detail: "launchctl bootout completed for both per-login domains".to_string(),
        ..RemoteReport::default()
    })
}

pub async fn reset_service_listener(
    target: &ComputeTarget,
    service: &ManagedService,
    probe_url: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_loopback_probe_url(probe_url)?;
    let port = url::Url::parse(probe_url)
        .map_err(|error| DeployError(format!("invalid service probe URL: {error}")))?
        .port()
        .ok_or_else(|| DeployError("service probe URL has no explicit port".to_string()))?;
    let body = LISTENER_RESET_BODY.replace("@PORT@", &shlex_quote(&port.to_string()));
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// Stop one managed service for a fenced recovery cutover. Unlike
/// [`retire_service`], this leaves the unit enabled and registered.
pub async fn stop_service(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    stop_service_with_password(target, service, None, runner).await
}

/// Stop a managed service, using the host account credential when the unit is
/// a system LaunchDaemon. The credential travels only on stdin to `sudo -S`.
pub async fn stop_service_with_password(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if UnitDomain::from_path(&service.path).requires_privileged_bootstrap() {
        let password = sudo_password.ok_or_else(|| {
            DeployError(format!(
                "{} on {} is a system LaunchDaemon and {} has no readable host-account password",
                service.unit_id(),
                service.host,
                target.name
            ))
        })?;
        validate_unit_id(service.unit_id())?;
        let qualified = format!("system/{}", service.unit_id());
        let recovery = format!("system/{}-recovery", service.unit_id());
        for job in [&qualified, &recovery] {
            let output = host_channel::run_program_with_stdin(
                target,
                &[
                    "/usr/bin/sudo",
                    "-S",
                    "-p",
                    "",
                    "/bin/launchctl",
                    "bootout",
                    job,
                ],
                &format!("{password}\n"),
                runner,
            )
            .await?;
            if !output.ok() {
                let detail =
                    host_channel::last_error_line(&output, "sudo or launchctl returned no detail");
                if !detail.contains("Could not find specified service")
                    && !detail.contains("No such process")
                {
                    return Err(DeployError(format!(
                        "privileged launchd stop failed on {} for {} with exit {}: {}",
                        target.name, job, output.code, detail
                    )));
                }
            }
        }
        let body = STOP_BODY.replace("@DISOWNED_SWEEP@", DISOWNED_SWEEP);
        let prelude = remote_prelude(service.unit_id(), "", &service.path)?;
        return run_remote_checked(
            target,
            &prelude,
            &body,
            &end_state(STOPPED_DESCRIBE, STOPPED_PROBE),
            runner,
        )
        .await;
    }
    let body = STOP_BODY.replace("@DISOWNED_SWEEP@", DISOWNED_SWEEP);
    let prelude = remote_prelude(service.unit_id(), "", &service.path)?;
    run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(STOPPED_DESCRIBE, STOPPED_PROBE),
        runner,
    )
    .await
}

/// `service retire` on one host: bootout / disable, files kept.
///
/// Unlike a command that is merely tested to be working, `retire` must verify
/// a postcondition: the unit is actually unloaded. If bootout/disable fails
/// or is ineffective, the report reflects the host's state, not the script's
/// exit status, and the caller correctly refuses to forget the declaration.
pub async fn retire_service(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if UnitDomain::from_path(&service.path).requires_privileged_bootstrap() {
        let stopped = stop_service_with_password(target, service, sudo_password, runner).await?;
        if !stopped.succeeded("stopped") {
            return Ok(stopped);
        }
        let password = sudo_password.ok_or_else(|| {
            DeployError(format!(
                "{} on {} is a system LaunchDaemon and {} has no readable host-account password",
                service.unit_id(),
                service.host,
                target.name
            ))
        })?;
        validate_unit_id(service.unit_id())?;
        let qualified = format!("system/{}", service.unit_id());
        let recovery = format!("system/{}-recovery", service.unit_id());
        for job in [&qualified, &recovery] {
            let output = host_channel::run_program_with_stdin(
                target,
                &[
                    "/usr/bin/sudo",
                    "-S",
                    "-p",
                    "",
                    "/bin/launchctl",
                    "disable",
                    job,
                ],
                &format!("{password}\n"),
                runner,
            )
            .await?;
            if !output.ok() {
                let detail =
                    host_channel::last_error_line(&output, "sudo or launchctl returned no detail");
                return Err(DeployError(format!(
                    "privileged launchd disable failed on {} for {} with exit {}: {}",
                    target.name, job, output.code, detail
                )));
            }
        }
    }
    let body = RETIRE_BODY.to_string();
    let prelude = remote_prelude(service.unit_id(), "", &service.path)?;
    run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(STOPPED_DESCRIBE, STOPPED_PROBE),
        runner,
    )
    .await
}

/// `service adopt`'s probe: does this unit actually exist on this host, and
/// what does the host call its file?
pub async fn probe_service(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let script = remote_script(unit, "", "", PROBE_BODY)?;
    run_remote(target, script, runner).await
}

/// The rendered unit spellings for a deployed service, plus the label they
/// share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployPlan {
    /// The launchd label, and the stem of the systemd unit name.
    pub label: String,
    /// The systemd unit name (`<label>.service`).
    pub unit: String,
    /// Absolute program path on the target host.
    pub program: String,
    /// The argument vector the unit declares, in the one-line spelling the
    /// host reads back out of an installed unit. [`ensure_service`] compares
    /// the two, and comparing two renderings of the same list is the only way
    /// "the unit already runs this" can be a fact rather than a hope.
    pub argv: String,
    /// The launchd agent, for a host whose per-login domain exists.
    pub darwin_unit: String,
    /// The same job as a launchd daemon, for the system domain — the only one
    /// an ssh login without an Aqua session can bootstrap into. Carries
    /// [`REMOTE_USER_PLACEHOLDER`] as well as [`REMOTE_HOME_PLACEHOLDER`].
    pub darwin_daemon_unit: String,
    pub linux_unit: String,
    /// Install this plan as a system LaunchDaemon on Darwin, regardless of
    /// where the host's declaration or the per-login fallback would place it.
    /// An always-on host with no graphical session has only `system` to run a
    /// service in: its user-domain units die with the login that never comes,
    /// and a plist left in `~/Library/LaunchAgents` there is a service that
    /// runs whenever nobody needs it. `ensure_service` addresses the daemon
    /// file when this is set; the privileged steps it needs are the ones
    /// [`crate::deploy::service::ManagedService::privileged_command`] spells.
    ///
    /// Set from the target by [`requires_daemon_domain`] at plan time, so
    /// `deploy`, `ensure` and the autonomy reconciler cannot disagree about
    /// the domain of the same unit on the same host. `service ensure
    /// --as-daemon` still turns it on for a host whose declaration does not
    /// yet say always-on; nothing turns it off.
    pub force_daemon: bool,
}

/// Render every unit spelling for a new managed service.
///
/// All of them come from `local_install::InstallPlan`, the renderer used by
/// `stado bootstrap --local`. The Darwin spellings carry a reserved
/// home placeholder that the remote installer replaces before launchd reads
/// the plist; this keeps logs in the remote account's owner-only Stado directory.
const REMOTE_HOME_PLACEHOLDER: &str = "__STADO_HOME__";

/// The account a system daemon runs as, resolved on the host: a plist in
/// `/Library/LaunchDaemons` is read by root, and a job with no `UserName`
/// would run the fleet's control binary as uid 0 against an account-owned
/// `~/.stado`.
const REMOTE_USER_PLACEHOLDER: &str = "__STADO_USER__";

/// The environment every managed unit carries, before whatever its own
/// declaration adds.
///
/// `HOME` and `STADO_CONFIG` are here because launchd sets neither for a job it
/// starts, and without them a Stado process falls off the end of
/// [`crate::config_file`]'s search order — `$STADO_CONFIG`,
/// `./stado.config.json`, `~/.config/stado/config.json`, `~/.stado/config.json`
/// — and runs on defaults. A coordinator that does that ticks forever against
/// an empty store: `stado service ensure` installed
/// `com.wisent.compute.service.stado-local-control-plane` on the always-on mac
/// with `PATH` as its only variable, and eleven consecutive ticks reaped no
/// expired lease and dispatched nothing while 55 pinned jobs sat in the store
/// it could not see. The catalog-backed units on the same host
/// (`com.wisent.always-on.stado-object-api`) carried `HOME`, `STADO_CONFIG` and
/// the storage keys, so one installer produced a working unit and the other did
/// not.
///
/// Both values ride the [`REMOTE_HOME_PLACEHOLDER`] the remote installer
/// substitutes, so the account is the host's answer and never this machine's.
/// The config path is the one `stado host config-set` writes and
/// `stado host config-show` reads.
///
/// A declaration wins over all three: an entry in `extra_env` replaces the
/// value in place rather than appending a second plist key for the same name.
fn base_unit_environment(path: &str, extra_env: &[(String, String)]) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOME".to_string(), REMOTE_HOME_PLACEHOLDER.to_string()),
        (
            "STADO_CONFIG".to_string(),
            format!("{REMOTE_HOME_PLACEHOLDER}/.config/stado/config.json"),
        ),
        ("PATH".to_string(), path.to_string()),
    ];
    for (variable, value) in extra_env {
        match env.iter_mut().find(|(name, _)| name == variable) {
            Some(existing) => existing.1 = value.clone(),
            None => env.push((variable.clone(), value.clone())),
        }
    }
    env
}

pub fn plan_deploy(
    target: &ComputeTarget,
    name: &str,
    program: &str,
    args: &[String],
) -> Result<DeployPlan, DeployError> {
    validate_service_name(name)?;
    let label = local_install::label(DEPLOY_KIND, name);
    plan_deploy_labelled(target, name, &label, program, args, &[])
}

/// [`plan_deploy`] at a label the declaration already carries.
///
/// `plan_deploy` mints `com.wisent.compute.service.<name>`, which is right for
/// a unit being created and wrong for one that already exists under another
/// label. Rendering the minted spelling for a declaration that says the unit
/// is `com.wisent.stado-resolver` installs a SECOND launchd job running the
/// same program, and two resolvers competing for one stable loopback port is
/// exactly the shape of outage this module was written after. A declaration
/// that names its own label is rendered at that label, so a declared service
/// is reinstallable from the document without becoming a second service.
pub fn plan_deploy_labelled(
    target: &ComputeTarget,
    name: &str,
    label: &str,
    program: &str,
    args: &[String],
    extra_env: &[(String, String)],
) -> Result<DeployPlan, DeployError> {
    validate_service_name(name)?;
    validate_service_name(label)?;
    validate_program(program)?;
    for arg in args {
        validate_unit_argument(arg)?;
    }
    // Environment rides into the rendered unit verbatim, so it is held to the
    // same shape the runtime env-file writer enforces: an exported name and a
    // single-line value. A line break here is a second plist key, not a value.
    for (variable, value) in extra_env {
        validate_env_variable(variable)?;
        if value.contains('\n') || value.contains('\r') {
            return Err(DeployError(format!(
                "environment variable {variable} carries a line break"
            )));
        }
    }
    let label = label.to_string();
    let render = |os: LocalOs| {
        let path = match os {
            LocalOs::Darwin => "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            LocalOs::Linux => "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        };
        let mut exec_args = Vec::with_capacity(args.len() + 1);
        exec_args.push(program.to_string());
        exec_args.extend(args.iter().cloned());
        InstallPlan {
            // Both spellings are rendered here and the host picks between them
            // (`ensure_unit_path` / the remote prelude), so this plan never
            // addresses a unit path itself and needs no account.
            daemon: None,
            name: name.to_string(),
            kind: DEPLOY_KIND.to_string(),
            os,
            label: label.clone(),
            exec_args,
            env: base_unit_environment(path, extra_env),
        }
    };
    let darwin = render(LocalOs::Darwin);
    let linux = render(LocalOs::Linux);
    let unit = linux
        .unit_path(Path::new(HOME_PREFIX))
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| label.clone());
    let remote_home = Path::new(REMOTE_HOME_PLACEHOLDER);
    let mut exec_args = Vec::with_capacity(args.len() + 1);
    exec_args.push(program.to_string());
    exec_args.extend(args.iter().cloned());
    let plan = DeployPlan {
        label: label.clone(),
        unit,
        program: program.to_string(),
        argv: exec_args.join(" "),
        darwin_unit: darwin.content(remote_home),
        darwin_daemon_unit: local_install::daemon_plist_text(
            &label,
            &exec_args,
            &darwin.env,
            &remote_home
                .join(".stado")
                .join("logs")
                .join(format!("{label}.log")),
            REMOTE_USER_PLACEHOLDER,
        ),
        linux_unit: linux.content(remote_home),
        force_daemon: requires_daemon_domain(target),
    };
    guard_heredoc(&plan.darwin_unit)?;
    guard_heredoc(&plan.darwin_daemon_unit)?;
    guard_heredoc(&plan.linux_unit)?;
    Ok(plan)
}

/// Retain an authored systemd definition whose lifecycle semantics cannot be
/// represented by the generic program/args/environment renderer.
///
/// The run declaration remains the authority for process ownership. Requiring
/// the definition's one `ExecStart` to match that declaration prevents an
/// opaque unit body from making lifecycle commands install a different
/// program than the registry says they manage. Declared environment is
/// materialized into the authored body in place, so `ensure --env` and a later
/// declaration-driven convergence have the same semantics as generated units.
/// An explicit program override replaces only `ExecStart`, retaining native
/// dependencies and startup conditions rather than discarding the unit body.
pub fn retain_systemd_unit(
    plan: &mut DeployPlan,
    definition: &str,
    environment: &[(String, String)],
    replace_program: bool,
) -> Result<String, DeployError> {
    let mut starts = definition.lines().filter_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == "ExecStart").then_some(value.trim())
    });
    let Some(exec_start) = starts.next() else {
        return Err(DeployError(
            "authored systemd unit carries no ExecStart".to_string(),
        ));
    };
    if starts.next().is_some() {
        return Err(DeployError(
            "authored systemd unit carries more than one ExecStart".to_string(),
        ));
    }
    if exec_start != plan.argv && !replace_program {
        return Err(DeployError(format!(
            "authored systemd unit starts {}, but the declaration says {}",
            py_str_repr(exec_start),
            py_str_repr(&plan.argv),
        )));
    }

    let mut remaining: BTreeMap<&str, &str> = environment
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let mut rendered = String::with_capacity(definition.len());
    let mut inserted = false;
    for line in definition.lines() {
        if let Some((_, assignment)) = line
            .split_once('=')
            .filter(|(name, _)| name.trim() == "Environment")
        {
            let Some((name, _)) = assignment.split_once('=') else {
                return Err(DeployError(format!(
                    "authored systemd unit has malformed environment line {}",
                    py_str_repr(line),
                )));
            };
            if let Some(value) = remaining.remove(name) {
                rendered.push_str("Environment=");
                rendered.push_str(name);
                rendered.push('=');
                rendered.push_str(value);
                rendered.push('\n');
            }
            continue;
        }
        if !inserted && line.trim_start().starts_with("ExecStart") {
            for (name, value) in &remaining {
                rendered.push_str("Environment=");
                rendered.push_str(name);
                rendered.push('=');
                rendered.push_str(value);
                rendered.push('\n');
            }
            remaining.clear();
            inserted = true;
        }
        if replace_program
            && line
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == "ExecStart")
        {
            rendered.push_str("ExecStart=");
            rendered.push_str(&plan.argv);
            rendered.push('\n');
            continue;
        }
        rendered.push_str(line);
        rendered.push('\n');
    }
    if !remaining.is_empty() {
        return Err(DeployError(
            "authored systemd unit carries no ExecStart position for its declared environment"
                .to_string(),
        ));
    }
    guard_heredoc(&rendered)?;
    plan.linux_unit = rendered.clone();
    Ok(rendered)
}

/// `service deploy` on one host: push the rendered unit and bootstrap it.
///
/// This is the fleet's only start, so it carries the start's end state. It
/// needs one more than restart does: the caller records the unit in the
/// canonical registry from this report, and a deploy that fell through to
/// `launchctl submit` or to a bare background process leaves no job under
/// the label it is about to be declared under. Recording that is how the
/// registry comes to hold a service the host has never heard of.
pub async fn deploy_service(
    target: &ComputeTarget,
    plan: &DeployPlan,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    // Delimiter first: substituting it after the unit bodies would let a
    // rendered unit that happens to contain the marker text be rewritten
    // into the delimiter itself. The trailing newline is trimmed because
    // the heredoc supplies one, so the file written on the host is
    // byte-identical to what `local_install` writes locally.
    let body = DEPLOY_BODY
        .replace("@HEREDOC@", UNIT_HEREDOC)
        .replace("@PROGRAM@", &shlex_quote(&plan.program))
        .replace("@DARWIN_UNIT@", plan.darwin_unit.trim_end_matches('\n'))
        .replace("@LINUX_UNIT@", plan.linux_unit.trim_end_matches('\n'));
    // The path is derived remotely from the unit id, which differs per OS,
    // so both spellings travel and the host picks.
    let prelude = remote_prelude(&plan.label, &plan.unit, "")?;
    let mut report = run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RUNNING_DESCRIBE, RUNNING_PROBE),
        runner,
    )
    .await?;
    report.name_unloaded(&plan.label, "deploy");
    Ok(report)
}

// ---------------------------------------------------------------------------
// Ensure: the unit a host must be running, asserted rather than installed
// ---------------------------------------------------------------------------

/// The unit was not there and this pass installed it.
pub const ACTION_CREATED: &str = "created";
/// The unit was there and this pass kicked it in place.
pub const ACTION_RESTARTED: &str = "restarted";
/// The unit was there, running the declared program, and nothing was touched.
pub const ACTION_ALREADY_CORRECT: &str = "already_correct";
/// The unit was there with the declared Program and argv, but its rendered file
/// had drifted; this pass installed and activated the desired definition through
/// the guarded init-system lifecycle. See the incident in [`ensure_service`]:
/// changing `base_unit_environment` to render `HOME` or `STADO_CONFIG` leaves
/// installed units with stale environments until this definition is reloaded.
/// On launchd that requires `bootout` then `bootstrap`, not an in-place kick.
pub const ACTION_CONVERGED: &str = "converged";
/// launchd held a stale program or argument vector and this pass reloaded the
/// already-preflighted definition, then verified launchd's own readback.
pub const ACTION_RELOADED: &str = "reloaded";

/// launchd's system domain: `/Library/LaunchDaemons`, reached with sudo, and
/// the only domain that exists on an ssh login with no Aqua session.
pub const DOMAIN_SYSTEM: &str = "system";
/// The per-login domain (`gui/<uid>` or `user/<uid>`), and `systemd --user`.
pub const DOMAIN_USER: &str = "user";

/// What one `ensure` pass found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureOutcome {
    /// [`ACTION_CREATED`], [`ACTION_RESTARTED`], [`ACTION_ALREADY_CORRECT`],
    /// [`ACTION_CONVERGED`] or [`ACTION_RELOADED`]; any other word is a failure
    /// the remote program named.
    pub action: String,
    /// The domain the unit ended up in, as launchd spells it.
    pub domain: String,
    /// The pid running under the unit after the pass, empty when none is.
    pub pid: String,
    /// The unit file the pass settled on. The body picks the domain, and
    /// therefore the path, after the prelude has already printed the one it
    /// derived, so this is the authority and not `STADO_HOST`'s field.
    pub path: String,
    pub report: RemoteReport,
}

impl EnsureOutcome {
    /// True when the pass reached one of the intended actions AND the host was
    /// observed with the unit loaded and running afterwards.
    ///
    /// [`ACTION_CONVERGED`] belongs here. It was added as a success action —
    /// a drifted unit file rewritten and activated through the guarded
    /// init-system lifecycle — but this set was never widened to admit it, so
    /// every converged pass was reported as
    /// a failure naming the unit path it had just settled on. That is what
    /// stopped the stado 0.13.11 release submission: its "Ensure the declared
    /// object service" step converged
    /// `com.wisent.always-on.stado-object-api` on charless-mac-mini and then
    /// failed with `could not ensure …: converged: /Library/LaunchDaemons/…`,
    /// so no product release could be submitted at all.
    pub fn succeeded(&self) -> bool {
        matches!(
            self.action.as_str(),
            ACTION_CREATED
                | ACTION_RESTARTED
                | ACTION_ALREADY_CORRECT
                | ACTION_CONVERGED
                | ACTION_RELOADED
        ) && self.report.postcondition_held()
    }

    /// The two-valued domain an operator acts on: a unit in the system domain
    /// needs sudo and survives a logout, a unit in the per-login one does
    /// neither. `systemd --user` is the same answer as launchd's per-login
    /// domain, because it is the same fact about who owns the job.
    pub fn domain_word(&self) -> &'static str {
        if self.domain == DOMAIN_SYSTEM {
            DOMAIN_SYSTEM
        } else {
            DOMAIN_USER
        }
    }

    /// True when this pass changed the host.
    pub fn changed(&self) -> bool {
        self.action != ACTION_ALREADY_CORRECT
    }
}

/// The unit file [`ensure_service`] addresses for this plan: the system
/// daemon path when the plan forces daemon placement, and otherwise the empty
/// path, which lets the remote prelude find an existing agent file before it
/// falls back to this login's `LaunchAgents`.
///
/// The distinction is a repair, not a preference. An always-on host that
/// nobody logs into graphically runs its declared services only while their
/// plists sit in `/Library/LaunchDaemons`; the same plist under
/// `~/Library/LaunchAgents` there names a job launchd cannot keep alive,
/// because the per-login domain it loads into exists solely inside sessions
/// nobody opens. `force_daemon` is how the plan carries that host fact
/// ([`requires_daemon_domain`]), and how `service ensure --as-daemon` carries
/// it for a host whose declaration has not caught up yet.
pub fn ensure_unit_path(plan: &DeployPlan) -> String {
    if plan.force_daemon {
        format!("/Library/LaunchDaemons/{}.plist", plan.label)
    } else {
        String::new()
    }
}

/// `service ensure` on one host: leave a matching loaded definition untouched,
/// kick a matching definition when needed, and perform one preflighted
/// definition reload when the on-disk unit or launchd's retained Program or
/// argv differs.
pub async fn ensure_service(
    target: &ComputeTarget,
    plan: &DeployPlan,
    runner: &Runner,
) -> Result<EnsureOutcome, DeployError> {
    // Delimiter first, for the reason [`deploy_service`] gives: substituting
    // it after the unit bodies would let a rendered unit containing the
    // marker text be rewritten into the delimiter itself.
    let body = ENSURE_BODY
        .replace("@HEREDOC@", UNIT_HEREDOC)
        .replace("@PROGRAM@", &shlex_quote(&plan.program))
        .replace("@ARGV@", &shlex_quote(&plan.argv))
        .replace(
            "@DARWIN_DAEMON_UNIT@",
            plan.darwin_daemon_unit.trim_end_matches('\n'),
        )
        .replace("@DARWIN_UNIT@", plan.darwin_unit.trim_end_matches('\n'))
        .replace("@LINUX_UNIT@", plan.linux_unit.trim_end_matches('\n'));
    let prelude = prelude_with(
        &plan.label,
        &plan.unit,
        &ensure_unit_path(plan),
        NO_DOMAIN_SYSTEM,
        None,
    )?;
    let mut report = run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RUNNING_DESCRIBE, RUNNING_PROBE),
        runner,
    )
    .await?;
    report.name_unloaded(&plan.label, "ensure");
    let (domain, pid, path) = parse_ensure(&report.stdout)
        .unwrap_or_else(|| (report.domain.clone(), String::new(), report.path.clone()));
    Ok(EnsureOutcome {
        action: report.status.clone(),
        domain,
        pid,
        path,
        report,
    })
}

/// The `STADO_ENSURE` marker: the domain, pid and unit path the body settled
/// on. Absent for every failure path, which is why it is an [`Option`].
fn parse_ensure(stdout: &str) -> Option<(String, String, String)> {
    stdout
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_ENSURE", domain, pid, path] => Some((
                (*domain).to_string(),
                (*pid).to_string(),
                (*path).to_string(),
            )),
            _ => None,
        })
}

/// The managed-service record an `ensure` should be declared under.
///
/// [`record_from_report`] with the ensured path substituted: the record has to
/// name the file the unit was actually installed at, and for a system daemon
/// that is `/Library/LaunchDaemons/<label>.plist` rather than the per-login
/// agent path the prelude derived before the body chose a domain. A record
/// naming a file the host does not have is a declaration no later command can
/// act on.
pub fn record_from_ensure(
    host: &str,
    name: &str,
    outcome: &EnsureOutcome,
    managed_since: &str,
) -> ManagedService {
    let mut record = record_from_report(host, None, name, &outcome.report, managed_since);
    if !outcome.path.is_empty() {
        record.path = outcome.path.clone();
    }
    record
}

// ---------------------------------------------------------------------------
// Which artefact the live process is executing
// ---------------------------------------------------------------------------

/// What one unit's live process is running, as the host reported it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunningProgram {
    /// The unit id the host addressed.
    pub unit: String,
    /// The pid under the unit, empty when nothing runs under it.
    pub pid: String,
    /// The program the unit's own file declares.
    pub declared: String,
    /// What that declaration resolves to today, once a `current` link in it
    /// has been followed.
    pub resolved: String,
    /// The executable the process table says the pid is running.
    pub running: String,
    /// When the process started.
    pub started_epoch: Option<i64>,
    /// When the declared program was last written.
    pub declared_written_epoch: Option<i64>,
    /// When the running executable was last written.
    pub running_written_epoch: Option<i64>,
}

impl RunningProgram {
    /// The executable path, or `None` when no process was found to ask about.
    pub fn running_binary(&self) -> Option<&str> {
        if self.running.is_empty() {
            None
        } else {
            Some(self.running.as_str())
        }
    }

    /// Whether the process is executing the artefact the unit's declaration
    /// resolves to, or `None` when that could not be established.
    ///
    /// Two ways for a loaded unit at the declared version to be running code
    /// nobody shipped, and both are production incidents:
    ///
    /// - The executable is not what the declaration resolves to. Brama's
    ///   process kept running an artefact tree that `current` no longer
    ///   pointed at, so the unit, the version on disk and the release were all
    ///   correct and the live process was none of them.
    /// - The file was written after the process started. The Weles worker kept
    ///   serving a `dist` that was replaced 26 seconds into its run: the path
    ///   still matches, and the artefact the process loaded is gone.
    ///
    /// `None` is never folded into either answer, for the reason
    /// `service converge` keeps `unknown` apart from `drifted`: a unit with
    /// nothing running under it, or a host that would not say when a file was
    /// written, has produced no evidence about artefact identity, and
    /// answering `true` there would be the report this field exists to
    /// replace.
    pub fn matches_process(&self) -> Option<bool> {
        if self.running.is_empty() || self.declared.is_empty() {
            return None;
        }
        if self.running != self.declared && self.running != self.resolved {
            return Some(false);
        }
        let started = self.started_epoch?;
        let written = match (self.declared_written_epoch, self.running_written_epoch) {
            (Some(declared), Some(running)) => declared.max(running),
            (Some(epoch), None) | (None, Some(epoch)) => epoch,
            (None, None) => return None,
        };
        Some(written <= started)
    }
}

/// Ask one host what the live process under one managed unit is executing.
pub async fn inspect_process(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RunningProgram, DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, PROCESS_BODY)?;
    let report = run_remote(target, script, runner).await?;
    if !report.succeeded("inspected") {
        return Err(DeployError(format!(
            "{}: could not inspect the process under {}: {}",
            target.name,
            service.unit_id(),
            report.failure()
        )));
    }
    let mut program = parse_process(&report.stdout);
    program.unit = report.unit.clone();
    Ok(program)
}

/// The `STADO_PROCESS` marker. An epoch the host could not read arrives empty
/// and stays `None`: a missing timestamp is the absence of a fact, and zero
/// would compare as 1970 and call every process stale.
fn parse_process(stdout: &str) -> RunningProgram {
    let mut program = RunningProgram::default();
    for line in stdout.lines() {
        if let ["STADO_PROCESS", pid, declared, resolved, running, started, declared_written, running_written] =
            host_channel::marker_fields(line).as_slice()
        {
            program.pid = (*pid).to_string();
            program.declared = (*declared).to_string();
            program.resolved = (*resolved).to_string();
            program.running = (*running).to_string();
            program.started_epoch = started.trim().parse().ok();
            program.declared_written_epoch = declared_written.trim().parse().ok();
            program.running_written_epoch = running_written.trim().parse().ok();
        }
    }
    program
}

// ---------------------------------------------------------------------------
// Which image a unit's live process is executing, on this machine
// ---------------------------------------------------------------------------

/// The three directories this fleet installs launchd units into, in the
/// order [`LOADED_LABELS_SCRIPT`] walks them.
///
/// Same list and same order deliberately: a unit one enumeration can see and
/// the other cannot is how a label ends up in nobody's set, which is the
/// defect `service list --undeclared` was built for.
const LAUNCHD_UNIT_DIRECTORIES: [&str; 3] = [
    "/Library/LaunchDaemons",
    "$HOME/Library/LaunchAgents",
    "/Library/LaunchAgents",
];

/// How long the file a unit declares must have been in place before a
/// process executing some other image counts as stale.
///
/// The tolerance exists because replacement and restart are two steps of one
/// invocation: [`crate::self_update::recycle_replaced_units`] writes the new
/// bytes and only afterwards walks the units to cycle them, so between those
/// two moments every managed process is legitimately still on the image it
/// started with. Firing there would report the installer's own working state
/// as a fault.
///
/// 300 seconds, from the only measurement of that window this fleet has.
/// `com.wisent.compute.disk-cleanup.disk-cleanup` journalled its last pass on
/// the superseded image at `2026-09-02T17:50:40Z` and its first pass on the
/// new one at `2026-09-02T17:51:35Z`: 55 seconds, and that figure already
/// contains a whole janitor pass rather than just the restart. Five times it
/// is a grace no legitimate replacement exhausts, and it is four orders of
/// magnitude short of the thirteen days that unit spent unnoticed, so the
/// tolerance costs this check nothing it was built to catch.
///
/// It is keyed on the age of the INSTALLED FILE and never on the age of the
/// process, which is the part that is easy to get backwards. A stale process
/// is old by construction — six days old, in the case this check exists for —
/// so suppressing young processes would suppress nothing and suppressing old
/// ones would suppress the finding. What is genuinely short-lived is the
/// replacement, and that is what this measures.
pub const IMAGE_SETTLE_SECONDS: i64 = 300;

/// One executable file, as the kernel identifies it rather than as a path
/// spells it.
///
/// A path is not an identity, and that gap is the whole condition this type
/// exists to express: two different files answering to one name. Every field
/// here is here because a path comparison cannot see it — which is why
/// [`RunningProgram::matches_process`], which compares paths and then
/// timestamps, reports a unit whose binary was swapped underneath it as
/// matching, and why `recycle_launchd` decides what to restart by string
/// equality on `argv[0]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageIdentity {
    /// Where the identity was read: the declared path for an installed file,
    /// and whatever the kernel still calls the mapping for a running one.
    pub path: String,
    /// `st_dev`. Inode numbers repeat across volumes, so the pair is the
    /// identity and the inode on its own is not.
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
    /// Directory entries pointing at this inode. Zero means the file has been
    /// unlinked and the running process holds the last reference to the bytes
    /// it is executing — a different operator problem from a process running
    /// some other file that still exists, so the two are never merged.
    pub links: u64,
}

impl ImageIdentity {
    /// The same file, by the only test that answers it.
    pub fn is_same_file(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode
    }

    /// The identity in one clause, so a report can print both sides and be
    /// believed without anybody going back to `lsof`.
    pub fn describe(&self) -> String {
        format!(
            "inode {} on device {:#x}, {} bytes, {} link(s)",
            self.inode, self.device, self.bytes, self.links
        )
    }
}

/// What a managed unit's live process turned out to be executing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageState {
    /// The executing file has no name left. The directory entry now points at
    /// other bytes and the running process holds the only remaining reference
    /// to the ones it is executing.
    ///
    /// This is the case that actually happened: on 2026-09-02 the janitor's
    /// six-day-old `--watch` process was executing an inode with zero links
    /// while `~/.stado/bin/stado` had been replaced more than once underneath
    /// it. It is a variant of its own because it is the one where no copy of
    /// the running build survives anywhere to be diffed.
    Unlinked {
        running: ImageIdentity,
        installed: ImageIdentity,
    },
    /// The executing file still exists and is not the one the unit declares —
    /// an artefact tree a `current` link no longer points at, or a second copy
    /// of the same program elsewhere on the disk.
    Replaced {
        running: ImageIdentity,
        installed: ImageIdentity,
    },
    /// The identity could not be established.
    ///
    /// A finding and never a silence. The defect this check exists to remove
    /// is an unread state rendered as a passing one, and `registry doctor`
    /// already applies the same rule to unit files it cannot open:
    /// [`EnvironmentGap::UnrecordedDeclaration`] carries `observed: None` and
    /// says the file was not read rather than printing an empty environment.
    Unread {
        /// What could not be read, named the way an operator would name it.
        subject: String,
        /// The reader's own words for why, never a paraphrase.
        reason: String,
    },
}

/// One managed unit whose live process is not executing the file the unit's
/// own `ProgramArguments` name — or one the question could not be asked about.
///
/// Measured on `lukasz-macbook` on 2026-09-02, and the measurement is why this
/// exists. `com.wisent.transcript-lake-stream` had been running pid 99986
/// since the previous afternoon on inode 125374164, 3,058,288 bytes, zero
/// links; the `/Users/lukaszbartoszcze/.local/bin/transcript-lake` its plist
/// names resolved to inode 181713431 at 2,958,720 bytes, written that morning.
/// Nothing in the fleet reported it and nothing could have:
/// `self_update::recycle_replaced_units` cycles a unit only as a side effect
/// of the invocation that replaced its bytes, matches by string equality on
/// `argv[0]`, compares no identity at all, and never revisits a unit it missed
/// or that a failed `kickstart` left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleUnitImage {
    pub host: String,
    /// launchd label. Empty on the row that reports a whole host unread,
    /// which is about no single unit.
    pub unit: String,
    /// The unit file the declaration was read from.
    pub unit_path: String,
    /// `ProgramArguments[0]`: the file the unit says it starts.
    pub program: String,
    pub pid: Option<u32>,
    /// How long the process has been alive.
    pub process_age_seconds: Option<i64>,
    /// How long ago the declared program was last written.
    pub installed_age_seconds: Option<i64>,
    pub state: ImageState,
}

impl StaleUnitImage {
    /// Stable machine-readable category, in `registry doctor`'s vocabulary.
    pub fn kind(&self) -> &'static str {
        match self.state {
            ImageState::Unlinked { .. } | ImageState::Replaced { .. } => "stale-unit-image",
            ImageState::Unread { .. } => "unread-unit-image",
        }
    }

    /// An age in the spelling every other `registry doctor` row uses.
    fn age(seconds: Option<i64>) -> String {
        seconds.map_or_else(
            || "an unread time".to_string(),
            |seconds| crate::cli::registry::human_age(TimeDelta::seconds(seconds)),
        )
    }

    /// The row an operator reads. It names the unit, the path and BOTH
    /// identities, because "stale" on its own sends somebody back to `lsof` to
    /// re-derive the two facts the check already holds.
    pub fn sentence(&self) -> String {
        let pid = self
            .pid
            .map_or_else(|| "no pid".to_string(), |pid| format!("pid {pid}"));
        match &self.state {
            ImageState::Unlinked { running, installed } => format!(
                "{} is running {pid}, started {} ago, and the executable that process is running \
                 has been unlinked: {}. Its ProgramArguments name {}, which now holds {}, written \
                 {} ago. No copy of the running build is left on disk, so the process is serving \
                 bytes nothing on this host can reproduce. Restarting the unit is what puts it on \
                 the installed file, and nothing does that on its own: \
                 self_update::recycle_replaced_units cycles units only inside the invocation that \
                 replaced them and never revisits one it missed",
                self.unit,
                Self::age(self.process_age_seconds),
                running.describe(),
                self.program,
                installed.describe(),
                Self::age(self.installed_age_seconds),
            ),
            ImageState::Replaced { running, installed } => format!(
                "{} is running {pid}, started {} ago, and the executable that process is running \
                 is not the file its unit declares: it is {} at {}. Its ProgramArguments name {}, \
                 which holds {}, written {} ago. Both files still exist, so the running one can be \
                 compared against the installed one before the unit is restarted",
                self.unit,
                Self::age(self.process_age_seconds),
                running.describe(),
                running.path,
                self.program,
                installed.describe(),
                Self::age(self.installed_age_seconds),
            ),
            ImageState::Unread { subject, reason } => format!(
                "{subject} could not be read, so whether the live process is executing the file \
                 its unit declares is unknown here and is NOT reported as agreement: {reason}"
            ),
        }
    }

    pub fn to_json(&self) -> Value {
        let identity = |image: &ImageIdentity| {
            json!({
                "path": image.path,
                "device": image.device,
                "inode": image.inode,
                "bytes": image.bytes,
                "links": image.links,
            })
        };
        let (state, running, installed, unread) = match &self.state {
            ImageState::Unlinked { running, installed } => (
                "unlinked",
                Some(identity(running)),
                Some(identity(installed)),
                None,
            ),
            ImageState::Replaced { running, installed } => (
                "replaced",
                Some(identity(running)),
                Some(identity(installed)),
                None,
            ),
            ImageState::Unread { subject, reason } => {
                ("unread", None, None, Some(format!("{subject}: {reason}")))
            }
        };
        json!({
            "host": self.host,
            "unit": self.unit,
            "unit_path": self.unit_path,
            "program": self.program,
            "pid": self.pid,
            "process_age_seconds": self.process_age_seconds,
            "installed_age_seconds": self.installed_age_seconds,
            "state": state,
            "running": running,
            "installed": installed,
            "unread_reason": unread,
        })
    }
}

/// Resolve Apple's `/bin/sh` dispatcher without duplicating its shell-selection
/// policy. Privileged mode ignores user startup files; the readiness byte keeps
/// the selected shell alive while the ordinary kernel image reader observes it.
#[cfg(target_os = "macos")]
fn selected_macos_shell() -> Result<PathBuf, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = Command::new("/bin/sh")
        .args(["-p", "-c", "printf R; read -r stado_image_continue"])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not resolve macOS /bin/sh: {error}"))?;
    let result = (|| {
        let mut ready = [0_u8; 1];
        child
            .stdout
            .as_mut()
            .ok_or_else(|| "macOS /bin/sh has no readiness pipe".to_string())?
            .read_exact(&mut ready)
            .map_err(|error| format!("macOS /bin/sh did not become observable: {error}"))?;
        if ready != *b"R" {
            return Err("macOS /bin/sh returned an unexpected readiness byte".to_string());
        }
        running_images(&[child.id()])?
            .remove(&child.id())
            .map(|image| PathBuf::from(image.path))
            .ok_or_else(|| "the shell selected by macOS /bin/sh has no readable image".to_string())
    })();
    // EOF releases the shell's read, including every failed observation.
    drop(child.stdin.take());
    child
        .wait()
        .map_err(|error| format!("could not reap the macOS shell image reader: {error}"))?;
    result
}

/// The identity of the file at `path` right now, and when it was last written.
///
/// Symlinks are followed, which is the point and not a convenience: a declared
/// program is routinely a link — `~/.local/bin/transcript-lake` is one, and
/// every staged release reaches its binary through a `current` link — and the
/// identity that matters is the one an `exec` of that path would land on
/// today.
pub fn installed_image(path: &Path) -> Result<(ImageIdentity, i64), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    let (path, metadata) = if std::fs::metadata("/bin/sh")
        .is_ok_and(|shell| shell.dev() == metadata.dev() && shell.ino() == metadata.ino())
    {
        let selected = selected_macos_shell()?;
        let selected_metadata = std::fs::metadata(&selected).map_err(|error| error.to_string())?;
        (std::borrow::Cow::Owned(selected), selected_metadata)
    } else {
        (std::borrow::Cow::Borrowed(path), metadata)
    };
    Ok((
        ImageIdentity {
            path: path.to_string_lossy().into_owned(),
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.size(),
            links: metadata.nlink(),
        },
        metadata.mtime(),
    ))
}

/// The image each of `pids` is executing, keyed by pid.
///
/// A pid absent from the returned map is one whose image this account could
/// not read — it exited, or it belongs to another user — and the caller
/// reports that as unknown rather than dropping it. `Err` is the reader itself
/// failing, which is one cause for every pid and is reported once.
pub fn running_images(pids: &[u32]) -> Result<BTreeMap<u32, ImageIdentity>, String> {
    if pids.is_empty() {
        return Ok(BTreeMap::new());
    }
    if cfg!(target_os = "linux") {
        Ok(proc_exe_images(pids))
    } else {
        lsof_images(pids)
    }
}

/// Linux: `/proc/<pid>/exe`. `read_link` names the image and appends
/// ` (deleted)` once it has been unlinked; `metadata` follows the magic link
/// to the inode itself, so it answers for a file with no directory entry left
/// exactly as it does for one that has one.
fn proc_exe_images(pids: &[u32]) -> BTreeMap<u32, ImageIdentity> {
    use std::os::unix::fs::MetadataExt;
    const DELETED: &str = " (deleted)";
    let mut images = BTreeMap::new();
    for &pid in pids {
        let link = PathBuf::from(format!("/proc/{pid}/exe"));
        let Ok(metadata) = std::fs::metadata(&link) else {
            continue;
        };
        let path = std::fs::read_link(&link).map_or_else(
            |_| format!("/proc/{pid}/exe"),
            |target| {
                let rendered = target.to_string_lossy().into_owned();
                rendered
                    .strip_suffix(DELETED)
                    .map_or_else(|| rendered.clone(), str::to_string)
            },
        );
        images.insert(
            pid,
            ImageIdentity {
                path,
                device: metadata.dev(),
                inode: metadata.ino(),
                bytes: metadata.size(),
                links: metadata.nlink(),
            },
        );
    }
    images
}

/// macOS: one `lsof` over every pid at once.
///
/// `-d txt` restricts the listing to text mappings and the FIRST of them is
/// the process's own executable; the rest are `dyld` and the frameworks it
/// pulled in. Some of those are routinely unlinked too — a `.plist-cache`
/// under `/Library/Preferences/Logging` is, on this machine — so "any unlinked
/// text mapping" is not the question and is not asked.
///
/// One invocation rather than one per pid: this runs inside a diagnostic that
/// already walks every unit on the host, and `host_disk` records what a
/// per-item `lsof` costs there.
fn lsof_images(pids: &[u32]) -> Result<BTreeMap<u32, ImageIdentity>, String> {
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<String>>()
        .join(",");
    // A pid that has exited makes lsof exit non-zero while still reporting
    // every pid that has not, so the status is deliberately not consulted: the
    // map is the answer, and an absent pid is already the unknown.
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &list, "-d", "txt", "-F", "pDsikn"])
        .output()
        .map_err(|error| format!("/usr/sbin/lsof did not run: {error}"))?;
    let rendered = String::from_utf8_lossy(&output.stdout);
    let mut images: BTreeMap<u32, ImageIdentity> = BTreeMap::new();
    let mut pid: Option<u32> = None;
    let mut current = ImageIdentity {
        path: String::new(),
        device: 0,
        inode: 0,
        bytes: 0,
        links: 0,
    };
    for line in rendered.lines() {
        let Some((tag, value)) = line.split_at_checked(1) else {
            continue;
        };
        match tag {
            "p" => pid = value.trim().parse().ok(),
            // lsof prints the device as `0x`-prefixed hex; the number is the
            // same `st_dev` `installed_image` reads, so the two sides compare
            // without a second convention.
            "D" => {
                current.device = value
                    .trim()
                    .strip_prefix("0x")
                    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                    .or_else(|| value.trim().parse().ok())
                    .unwrap_or_default();
            }
            "s" => current.bytes = value.trim().parse().unwrap_or_default(),
            "i" => current.inode = value.trim().parse().unwrap_or_default(),
            "k" => current.links = value.trim().parse().unwrap_or_default(),
            "n" => {
                current.path = value.to_string();
                if let Some(pid) = pid {
                    images.entry(pid).or_insert_with(|| current.clone());
                }
            }
            _ => {}
        }
    }
    Ok(images)
}

/// Every process this account can see: pid, how long it has been alive, and
/// the argument vector, which is what joins a process back to the unit that
/// declares it.
///
/// `etime` and not `etimes`: macOS `ps` has no `etimes` keyword at all, and
/// asking for one makes it reject the entire format string and print an
/// unlabelled table instead of failing.
pub fn process_table() -> Result<Vec<(u32, Option<i64>, String)>, String> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,etime=,args="])
        .output()
        .map_err(|error| format!("/bin/ps did not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "/bin/ps exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let rendered = String::from_utf8_lossy(&output.stdout);
    let mut rows = Vec::new();
    for line in rendered.lines() {
        let Some((pid, rest)) = line.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let Some((elapsed, argv)) = rest.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        rows.push((pid, parse_etime(elapsed), argv.trim().to_string()));
    }
    Ok(rows)
}

/// `ps` elapsed time — `[[dd-]hh:]mm:ss` — in seconds.
fn parse_etime(elapsed: &str) -> Option<i64> {
    let (days, clock) = elapsed
        .split_once('-')
        .map_or((0i64, elapsed), |(days, clock)| {
            (days.trim().parse().unwrap_or_default(), clock)
        });
    let mut seconds = 0i64;
    for field in clock.split(':') {
        seconds = seconds * 60 + field.trim().parse::<i64>().ok()?;
    }
    Some(days * 86_400 + seconds)
}

/// Every launchd unit on THIS machine that this fleet is answerable for,
/// keyed by label and valued by the unit file to read.
///
/// Two sources, because either alone has a blind spot this check cannot
/// afford. The registry's own `services` array is the declared set — and
/// `com.wisent.compute.disk-cleanup.disk-cleanup`, the unit the whole incident
/// happened to, is not in it on `lukasz-macbook`. The three unit directories
/// carry every label this fleet installed whether the document adopted it or
/// not, which is the class [`UndeclaredUnit::fleet_affiliated`] was widened to
/// see, and they miss a declared unit whose file has been deleted. The union
/// is what the fleet is answerable for.
fn local_launchd_units(target: &ComputeTarget, home: &str) -> BTreeMap<String, String> {
    let mut units: BTreeMap<String, String> = BTreeMap::new();
    for service in declared_services(target) {
        if service.kind != KIND_LAUNCHD || service.path.is_empty() {
            continue;
        }
        units.insert(
            service.unit_id().to_string(),
            service.path.replace("$HOME", home),
        );
    }
    for directory in LAUNCHD_UNIT_DIRECTORIES {
        let Ok(entries) = std::fs::read_dir(PathBuf::from(directory.replace("$HOME", home))) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Exactly `<label>.plist`. The disabled, retired and dated
            // siblings beside them — `.plist.stado-disabled`,
            // `.plist.retired-20260818` — are not units launchd loads, and
            // reporting on them would be reporting on files nobody runs.
            let Some(label) = name.strip_suffix(".plist") else {
                continue;
            };
            if !label.starts_with(FLEET_LABEL_PREFIX) {
                continue;
            }
            units
                .entry(label.to_string())
                .or_insert_with(|| entry.path().to_string_lossy().into_owned());
        }
    }
    units
}

/// Whether a running image is a finding, given the file the unit declares.
///
/// `None` for the two states that are not: the same file, and a replacement
/// young enough to still be mid-flight. Pure, so the boundary can be exercised
/// without a process to point it at.
pub fn classify_image(
    running: &ImageIdentity,
    installed: &ImageIdentity,
    installed_age_seconds: i64,
) -> Option<ImageState> {
    if running.is_same_file(installed) || installed_age_seconds < IMAGE_SETTLE_SECONDS {
        return None;
    }
    let (running, installed) = (running.clone(), installed.clone());
    if running.links == 0 {
        return Some(ImageState::Unlinked { running, installed });
    }
    Some(ImageState::Replaced { running, installed })
}

/// One managed unit's image, as read on the machine holding its process —
/// including the units that turned out to be fine.
///
/// Two callers need this and they must never disagree: `registry doctor`
/// reports the units that are stale, and `service refresh-image` refuses to
/// act on a unit that is not. A refusal has to name the identity it found, so
/// the clean answer is a value here rather than an absence, and the finding is
/// derived from it by [`UnitImageObservation::finding`] instead of being
/// produced by a second pass that could drift from the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitImageObservation {
    pub host: String,
    /// launchd label. Empty on the observation that covers a whole host.
    pub unit: String,
    pub unit_path: String,
    /// `ProgramArguments[0]`: the file the unit says it starts.
    pub program: String,
    pub pid: Option<u32>,
    pub process_age_seconds: Option<i64>,
    pub installed_age_seconds: Option<i64>,
    /// The image the live process is executing, when it was read.
    pub running: Option<ImageIdentity>,
    /// The file the unit declares, as it stands now.
    pub installed: Option<ImageIdentity>,
    /// `None` when the process is executing the file the unit declares, or
    /// when the replacement is still inside [`IMAGE_SETTLE_SECONDS`].
    pub state: Option<ImageState>,
}

impl UnitImageObservation {
    /// The `registry doctor` row for this observation, or `None` when there is
    /// nothing to report.
    pub fn finding(&self) -> Option<StaleUnitImage> {
        Some(StaleUnitImage {
            host: self.host.clone(),
            unit: self.unit.clone(),
            unit_path: self.unit_path.clone(),
            program: self.program.clone(),
            pid: self.pid,
            process_age_seconds: self.process_age_seconds,
            installed_age_seconds: self.installed_age_seconds,
            state: self.state.clone()?,
        })
    }

    /// Whether the running image and the declared file are the same file.
    ///
    /// `None` while either identity is unread, which is the answer this whole
    /// module exists to keep apart from `true`.
    pub fn agrees(&self) -> Option<bool> {
        Some(
            self.running
                .as_ref()?
                .is_same_file(self.installed.as_ref()?),
        )
    }
}

/// One row from the unit-image scan plus the native owner's observed argv.
///
/// The public observation predates the release revisit pass and remains the
/// stable value consumed by doctor and the manual refresh command. The
/// release pass additionally needs the subcommand to exclude units that
/// recycle themselves. Keeping it beside the observation internally preserves
/// that evidence from the same native process observation without widening the
/// public struct or reading either source again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnitImageScan {
    pub observation: UnitImageObservation,
    pub arguments: Vec<String>,
}

/// Every managed unit on one host with the image its live process is
/// executing.
///
/// LOCAL ONLY, and the signature says so. Which image a pid is executing is a
/// question only the kernel holding that pid can answer: nothing in the store
/// carries it, the beacon publishes one `state` word per unit, and
/// [`ManagedService`] records a path rather than an identity. So `local_units`
/// is the name of the host this process is running on, exactly as
/// [`unreachable_product_environments`] uses it, and every other host gets one
/// observation saying its units were not measured. That row is deliberate: the
/// state this check exists to remove is an unread one rendered as passing, and
/// a remote host silently omitted would be that same defect wearing this
/// check's name.
///
/// The native manager supplies each label's live PID. The unit file supplies
/// the installed program to compare with that PID's kernel image; its argv may
/// already differ from launchd's cached definition. An unloaded or stopped
/// unit holds no image, while ambiguous ownership remains explicitly unread.
pub(crate) async fn observe_unit_image_scan(
    target: &ComputeTarget,
    local_units: Option<&str>,
    now_epoch: i64,
) -> Vec<UnitImageScan> {
    let blank = |unit: &str, unit_path: &str, state: Option<ImageState>| UnitImageScan {
        observation: UnitImageObservation {
            host: target.name.clone(),
            unit: unit.to_string(),
            unit_path: unit_path.to_string(),
            program: String::new(),
            pid: None,
            process_age_seconds: None,
            installed_age_seconds: None,
            running: None,
            installed: None,
            state,
        },
        // No argv: the native owner or declared program could not be read.
        arguments: Vec::new(),
    };
    let whole_host = |reason: String| {
        blank(
            "",
            "",
            Some(ImageState::Unread {
                subject: format!("the executing image of every unit on {}", target.name),
                reason,
            }),
        )
    };
    if local_units != Some(target.name.as_str()) {
        let declared = declared_services(target).len();
        if declared == 0 {
            return Vec::new();
        }
        return vec![whole_host(format!(
            "which image a process is executing is readable only on the machine holding that \
             process, and this command is running on {}; {declared} declared unit(s) on {} are \
             unmeasured until `stado registry doctor` runs there",
            local_units.unwrap_or("a host no registry target names"),
            target.name
        ))];
    }
    let Some(home) = std::env::var_os("HOME").map(|home| home.to_string_lossy().into_owned())
    else {
        return vec![whole_host(
            "this process has no HOME, so the launchd unit directories could not be named"
                .to_string(),
        )];
    };
    let native_units = match loaded_units(target, &super::production_runner()).await {
        Ok(units) => units,
        Err(error) => return vec![whole_host(error.to_string())],
    };
    let native_by_label = native_units
        .iter()
        .map(|unit| (unit.label.as_str(), unit))
        .collect::<BTreeMap<_, _>>();

    // One pass over the unit files, then one image read for every pid they
    // name.
    let mut rows: Vec<UnitImageScan> = Vec::new();
    // Keep the native owner's PID and argv beside the declared executable.
    struct Matched {
        label: String,
        unit_path: String,
        program: String,
        arguments: Vec<String>,
        pid: u32,
        age: Option<i64>,
    }
    let mut pending: Vec<Matched> = Vec::new();
    for (label, unit_path) in local_launchd_units(target, &home) {
        let unread = |subject: String, reason: String| {
            blank(
                &label,
                &unit_path,
                Some(ImageState::Unread { subject, reason }),
            )
        };
        let Some(unit) = local_unit_file(&unit_path, KIND_LAUNCHD) else {
            rows.push(unread(
                format!("{label}'s unit file {unit_path}"),
                "it is absent, unreadable, or not a plist this build can parse".to_string(),
            ));
            continue;
        };
        if unit.arguments.is_empty() {
            rows.push(unread(
                format!("{label}'s declared program"),
                format!(
                    "{unit_path} carries neither ProgramArguments nor Program, so there is no \
                     declared file for a running image to be compared against"
                ),
            ));
            continue;
        }
        let Some(native) = native_by_label.get(label.as_str()) else {
            continue;
        };
        if native.loaded_domains.len() > 1 {
            rows.push(unread(
                format!("{label}'s native owner"),
                format!(
                    "launchd reports {} loaded domains; refusing to choose a process",
                    native.loaded_domains.len()
                ),
            ));
            continue;
        }
        let Ok(pid) = native.pid.parse::<u32>() else {
            continue;
        };
        if native.loaded_domains.is_empty() || native.running_program.is_empty() {
            rows.push(unread(
                format!("{label}'s native owner"),
                "a live PID has no readable owner domain or argument vector".to_string(),
            ));
            continue;
        }
        pending.push(Matched {
            label,
            unit_path,
            program: unit.program,
            arguments: native
                .running_program
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            pid,
            age: native.started_epoch.map(|started| now_epoch - started),
        });
    }

    let pids: Vec<u32> = pending.iter().map(|matched| matched.pid).collect();
    let images = match running_images(&pids) {
        Ok(images) => images,
        Err(reason) => {
            // One row, not one per pid: the cause is a reader that would not
            // answer, and it is the same cause for every process.
            rows.push(whole_host(reason));
            rows.sort_by(|left, right| left.observation.unit.cmp(&right.observation.unit));
            return rows;
        }
    };

    for matched in pending {
        let Matched {
            label,
            unit_path,
            program,
            arguments,
            pid,
            age,
        } = matched;
        let installed_read = installed_image(Path::new(&program));
        let mut scan = UnitImageScan {
            observation: UnitImageObservation {
                host: target.name.clone(),
                unit: label,
                unit_path,
                program,
                pid: Some(pid),
                process_age_seconds: age,
                installed_age_seconds: None,
                running: images.get(&pid).cloned(),
                installed: None,
                state: None,
            },
            arguments,
        };
        let row = &mut scan.observation;
        let (installed, written_epoch) = match installed_read {
            Ok(read) => read,
            Err(reason) => {
                row.state = Some(ImageState::Unread {
                    subject: format!("{}'s declared program {}", row.unit, row.program),
                    reason,
                });
                rows.push(scan);
                continue;
            }
        };
        let installed_age = now_epoch - written_epoch;
        row.installed_age_seconds = Some(installed_age);
        row.installed = Some(installed.clone());
        let Some(running) = row.running.clone() else {
            row.state = Some(ImageState::Unread {
                subject: format!("the image pid {pid} is executing for {}", row.unit),
                reason: "no text mapping was readable for that pid: it exited between the \
                         process listing and this read, or it belongs to another account"
                    .to_string(),
            });
            rows.push(scan);
            continue;
        };
        row.state = classify_image(&running, &installed, installed_age);
        rows.push(scan);
    }
    rows.sort_by(|left, right| {
        left.observation
            .unit
            .cmp(&right.observation.unit)
            .then(left.observation.pid.cmp(&right.observation.pid))
    });
    rows
}

/// The stable public projection of [`observe_unit_image_scan`].
///
/// Both callers receive rows produced by the same native-owner/image pass;
/// only the internal release revisit keeps the observed argv it additionally
/// needs.
pub async fn observe_unit_images(
    target: &ComputeTarget,
    local_units: Option<&str>,
    now_epoch: i64,
) -> Vec<UnitImageObservation> {
    observe_unit_image_scan(target, local_units, now_epoch)
        .await
        .into_iter()
        .map(|scan| scan.observation)
        .collect()
}

/// Managed units on one host whose live process is executing an image that is
/// not the file their `ProgramArguments` name.
///
/// The `registry doctor` view of [`observe_unit_images`]: the same pass, with
/// the units that are fine dropped.
pub async fn units_running_replaced_images(
    target: &ComputeTarget,
    local_units: Option<&str>,
    now_epoch: i64,
) -> Vec<StaleUnitImage> {
    observe_unit_images(target, local_units, now_epoch)
        .await
        .iter()
        .filter_map(UnitImageObservation::finding)
        .collect()
}

/// Restart one local launchd unit in its observed owner domain.
///
/// Reuse the in-place kick when launchd still holds the program on disk.
/// A changed cached definition must instead be reloaded through the existing
/// system or user service lifecycle, after the plist and executable are read.
/// Multiple owners, a domain inconsistent with the unit path, or an unreadable
/// replacement refuse before mutation. System operations remain non-interactive.
pub async fn restart_local_unit(
    target: &ComputeTarget,
    label: &str,
    unit_path: &str,
    observed_domain: Option<&str>,
) -> Result<String, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !host_channel::target_is_this_host(target) {
        return Err("local unit restart requires this host's registry target".to_string());
    }
    validate_unit_id(label).map_err(|error| error.to_string())?;
    let unit_domain = UnitDomain::from_path(unit_path);
    if matches!(unit_domain, UnitDomain::Unknown) {
        return Err(format!(
            "{unit_path} is in none of launchd's three unit directories, so no domain places it"
        ));
    }
    let mut candidates: Vec<String> = if unit_domain.requires_privileged_bootstrap() {
        vec!["system".to_string()]
    } else {
        let home = std::env::var_os("HOME").ok_or("this process has no HOME")?;
        let uid = std::fs::metadata(&home)
            .map_err(|error| format!("this account's uid is unreadable: {error}"))?
            .uid();
        vec![format!("gui/{uid}"), format!("user/{uid}")]
    };
    if let Some(observed) = observed_domain {
        if !candidates.iter().any(|candidate| candidate == observed) {
            return Err(format!(
                "{unit_path} permits {}, but launchd reports owner {observed}; refusing before restart",
                candidates.join(" or ")
            ));
        }
        candidates.retain(|candidate| candidate == observed);
    }
    let runner = super::production_runner();
    let units = loaded_units(target, &runner)
        .await
        .map_err(|error| error.to_string())?;
    let loaded = units
        .iter()
        .find(|unit| unit.label == label)
        .ok_or_else(|| format!("launchd holds no observed unit named {label}"))?;
    let [domain] = loaded.loaded_domains.as_slice() else {
        return Err(format!(
            "{label} has {} loaded owners; refusing to choose a lifecycle domain",
            loaded.loaded_domains.len()
        ));
    };
    if !candidates.contains(domain) {
        return Err(format!(
            "{unit_path} permits {}, but launchd reports owner {domain}",
            candidates.join(" or ")
        ));
    }
    let service = ManagedService {
        host: target.name.clone(),
        name: label.to_string(),
        label: label.to_string(),
        path: unit_path.to_string(),
        kind: KIND_LAUNCHD.to_string(),
        ..ManagedService::default()
    };
    let unit = fetch_unit_file(target, &service, &runner)
        .await
        .map_err(|error| error.to_string())?;
    let program = parse_unit_program(&unit)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{unit_path} declares no executable program"))?;
    let metadata = std::fs::metadata(&program)
        .map_err(|error| format!("cannot read the replacement {program}: {error}"))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!("{program} is not an executable file"));
    }
    let scope = if unit_domain.requires_privileged_bootstrap() {
        BootoutScope::System
    } else {
        BootoutScope::User
    };
    let cached = super::service_label_print::print_label(target, label, scope, &runner)
        .await
        .map_err(|error| error.to_string())?;
    if cached.domain.as_deref() != Some(domain.as_str()) {
        return Err(format!("{label} changed its loaded owner before restart"));
    }
    let cached_program = cached
        .program
        .as_deref()
        .or_else(|| cached.arguments.as_deref()?.split_whitespace().next())
        .ok_or_else(|| format!("{domain}/{label} has no readable cached program"))?;
    if cached_program != program
        || cached
            .arguments
            .as_deref()
            .is_some_and(|argv| argv != loaded.program)
    {
        let report = if unit_domain.requires_privileged_bootstrap() {
            reload_service_with_password(target, &service, None, &runner).await
        } else {
            restart_non_system_service(target, &service, Some(domain), true, &runner).await
        }
        .map_err(|error| error.to_string())?;
        if !report.succeeded("restarted") {
            return Err(report.failure());
        }
        if report.domain != *domain {
            return Err(format!(
                "{label} reloaded in {}, not {domain}",
                report.domain
            ));
        }
        return Ok(format!("{domain}/{label}"));
    }
    let qualified = format!("{domain}/{label}");
    let output = if unit_domain.requires_privileged_bootstrap() {
        std::process::Command::new("/usr/bin/sudo")
            .args(["-n", "/bin/launchctl", "kickstart", "-k", &qualified])
            .output()
            .map_err(|error| format!("/usr/bin/sudo did not run: {error}"))?
    } else {
        std::process::Command::new("/bin/launchctl")
            .args(["kickstart", "-k", &qualified])
            .output()
            .map_err(|error| format!("/bin/launchctl did not run: {error}"))?
    };
    if output.status.success() {
        Ok(qualified)
    } else {
        Err(format!(
            "{qualified}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

// ---------------------------------------------------------------------------
// Product processes no unit owns
// ---------------------------------------------------------------------------

/// Where `service deploy --from-artifact` installs the programs it renders
/// units around. Not a product root — the product declaration cannot name it,
/// because what lands there is whatever an operator deployed — and a program
/// running out of it is this fleet's program all the same.
pub const DEPLOYED_SERVICES_ROOT: &str = "$HOME/.stado/services";

/// What [`product_guess`] says about a command line that matches a managed
/// root and no product in the declaration.
pub const UNKNOWN_PRODUCT: &str = "unknown";

/// The `$HOME`-relative roots a managed program can execute out of: every
/// declared product's install root, plus [`DEPLOYED_SERVICES_ROOT`].
///
/// This is the whole definition of "a product process" for
/// [`unowned_processes`]. It comes off the shipped product declaration rather
/// than a list in this file, so a product added there is scanned for without a
/// matching edit here.
pub fn managed_roots() -> Result<Vec<String>, DeployError> {
    let mut roots = vec![DEPLOYED_SERVICES_ROOT.to_string()];
    for product in super::products::declared()? {
        let root = product.root().to_string();
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// Which product a command line belongs to.
///
/// The host reports absolute paths and the declaration is `$HOME`-relative, so
/// both are matched on the tail they share. A program product is identified by
/// its own file name and not by its root: `stado` and `skarbiec` install into
/// the same `$HOME/.stado/bin`, and reporting a four-day-old unowned agent as
/// possibly-skarbiec would be worse than saying nothing.
pub fn product_guess(command: &str) -> String {
    let tail = |root: &str| root.strip_prefix(HOME_PREFIX).unwrap_or(root).to_string();
    let deployed = format!("{}/", tail(DEPLOYED_SERVICES_ROOT));
    if let Some((_, rest)) = command.split_once(deployed.as_str()) {
        let name = rest.split(['/', ' ']).next().unwrap_or_default();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    let products = super::products::declared().unwrap_or_default();
    for product in products {
        if command.contains(&format!("{}/{}", tail(product.root()), product.name)) {
            return product.name.clone();
        }
    }
    for product in products {
        if command.contains(&format!("{}/", tail(product.root()))) {
            return product.name.clone();
        }
    }
    UNKNOWN_PRODUCT.to_string()
}

/// One product process on one host that no unit owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnownedProcess {
    pub host: String,
    pub pid: String,
    /// The full command line, tabs and newlines flattened on the host so one
    /// process can never span two marker lines.
    pub command: String,
    /// The host's own `ps` start stamp. Kept verbatim: four days is the fact
    /// that mattered on the always-on mac, and a reformatting that failed
    /// would report a process with no age at all.
    pub started_at: String,
}

impl UnownedProcess {
    pub fn product_guess(&self) -> String {
        product_guess(&self.command)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "pid": self.pid,
            "command": self.command,
            "started_at": self.started_at,
            "product_guess": self.product_guess(),
        })
    }
}

/// What one host's unowned-process scan searched, beside what it found.
///
/// The result alone could not be read. An empty `processes` meant either that
/// the host runs nothing unowned or that every root expanded to a path no
/// process could run out of, and those need opposite responses. This carries
/// the roots as the host expanded them, how many pids each one matched, how
/// many of those actually execute out of it, and how many pids launchd claimed
/// — so an empty answer states why it is empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnownedScan {
    pub processes: Vec<UnownedProcess>,
    /// `(root, pids matched by pgrep, pids executing out of the root)`.
    pub roots: Vec<(String, usize, usize)>,
    /// Pids launchd reported as owned across every printable domain.
    pub owned_pids: usize,
    /// `(pid, "owned"|"unowned", the ancestor pid launchd claimed)` for every
    /// candidate that executes out of a managed root. The verdict without its
    /// evidence is what made an empty table unreadable.
    pub judged: Vec<(String, String, String)>,
}

impl UnownedScan {
    /// One line an operator can read beside an empty table.
    pub fn account(&self, host: &str) -> String {
        let roots = self
            .roots
            .iter()
            .map(|(root, matched, under)| format!("{root} matched {matched}, under {under}"))
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "{host}: launchd claimed {} pid(s); {}",
            self.owned_pids,
            if roots.is_empty() {
                "no root was searched".to_string()
            } else {
                roots
            }
        )
    }
}

/// Every product process on one host that no launchd job or systemd unit owns,
/// with an account of what was searched to find them.
///
/// Read-only: it starts nothing, stops nothing and signals nothing, so it is
/// safe to run against a live production host.
pub async fn unowned_processes(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<UnownedScan, DeployError> {
    let mut roots = Vec::new();
    for root in managed_roots()? {
        // The roots keep `$HOME` unexpanded on this side and expanded on
        // theirs, so the same rule `quote_unit_path` applies to a declared
        // unit path applies to them: a vetted charset inside double quotes.
        roots.push(format!("\"{}\"", quote_unit_path(&root)?));
    }
    let script = UNOWNED_SCRIPT.replace("@ROOTS@", &roots.join(" "));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the unowned-process scan did not complete",
        )));
    }
    let mut scan = UnownedScan {
        processes: parse_unowned(&target.name, &output.stdout),
        ..Default::default()
    };
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_UNOWNED_OWNED", count] => {
                scan.owned_pids = count.trim().parse().unwrap_or_default();
            }
            ["STADO_UNOWNED_ROOT", root, matched, under] => scan.roots.push((
                (*root).trim().to_string(),
                matched.trim().parse().unwrap_or_default(),
                under.trim().parse().unwrap_or_default(),
            )),
            ["STADO_UNOWNED_JUDGED", pid, verdict, owner] => scan.judged.push((
                (*pid).trim().to_string(),
                (*verdict).trim().to_string(),
                (*owner).trim().to_string(),
            )),
            _ => {}
        }
    }
    Ok(scan)
}

/// The `STADO_UNOWNED` marker lines, in the order the host printed them.
fn parse_unowned(host: &str, stdout: &str) -> Vec<UnownedProcess> {
    stdout
        .lines()
        .filter_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_UNOWNED", pid, started, command] => Some(UnownedProcess {
                host: host.to_string(),
                pid: (*pid).to_string(),
                command: (*command).trim().to_string(),
                started_at: (*started).trim().to_string(),
            }),
            _ => None,
        })
        .collect()
}

/// The label prefix every unit this fleet installs carries, whichever writer
/// installed it: `local_install::LABEL_PREFIX` mints
/// `com.wisent.compute.<kind>.<name>` and the always-on set is
/// `com.wisent.always-on.<name>`, so one prefix covers both.
///
/// It NAMES a finding and never decides what gets looked at. It used to do
/// both, in three places at once — the `launchctl list` filter in
/// [`LOADED_LABELS_SCRIPT`], that script's `com.wisent.*.plist` glob, and a
/// `starts_with` in [`loaded_units`] — and a process outside the prefix could
/// therefore not be reported as undeclared, because it was never enumerated.
/// On 2026-09-01 charless-mac-mini had `com.stado.agent.charless-mac-mini`
/// loaded, the only label on the host outside `com.wisent.`, holding the pid
/// that was overwriting the janitor's state file — and
/// `service list --undeclared` answered that the host had no undeclared unit.
/// That answer was true about a window and false about the host.
///
/// This is the same shape as every other defect this module records: a
/// declaration checked against something narrower than the world. The fix is
/// not a wider prefix, because any prefix has an outside. The enumeration
/// walks every loaded label and every unit file, and the prefix survives only
/// as [`UndeclaredUnit::classification`] — `undeclared` and
/// `outside-fleet-prefix` are different sentences about a row, not different
/// decisions about whether to look.
const FLEET_LABEL_PREFIX: &str = "com.wisent.";

/// One launchd job loaded on a host, with everything the host could say about
/// it. The registry decides whether it is declared; the label's spelling
/// decides nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredUnit {
    pub host: String,
    pub label: String,
    /// Does the registry declare a service at this exact label on this host?
    ///
    /// The one question that decides whether a loaded job is accounted for.
    /// It is asked of the registry document, never of the label's spelling.
    pub declared: bool,
    /// The pid launchd holds for the label, or empty when it holds none.
    pub pid: String,
    /// The label's last exit status as launchd reports it.
    pub status: String,
    /// The unit file the host found for the label, or empty when it found none.
    pub path: String,
    /// Where `path` came from: `fleet-directory` for one of the three launchd
    /// directories this fleet installs into, `launchd` when only launchd knew
    /// and the host had to ask it, empty when no unit file was found at all.
    ///
    /// `com.stado.agent.charless-mac-mini` is loaded on charless-mac-mini from
    /// none of those three directories, so every reader that looked only there
    /// saw a label with no file and no program behind it.
    pub path_source: String,
    /// The argument vector that unit file declares, flattened to one line. A
    /// label alone is not actionable: three naming conventions produce three
    /// unrelated-looking labels for one job, and only the program says they are
    /// the same job.
    pub program: String,
    /// EVERY unit file found for this label, across the system-daemon, user-agent
    /// and system-agent domains. `path` is the first of these and stays what it
    /// was; this is the list, because more than one entry means one label is
    /// declared in more than one domain and launchd will happily run both.
    pub declaring_paths: Vec<String>,
    /// Every launchd domain that actually HOLDS this label, from that domain's
    /// own service table rather than from `launchctl list`.
    ///
    /// Empty means launchd holds no job under the label anywhere this login
    /// can print. Non-empty with no `declaring_paths` is the state that hid
    /// the respawner of #286: loaded, restarting, and in no directory.
    pub loaded_domains: Vec<String>,
    /// How many times launchd has started this job.
    ///
    /// A one-shot with `KeepAlive` does not read as broken anywhere else: it
    /// reads `active`, exits, and is restarted forever. The count is the only
    /// number that says so.
    pub runs: Option<u64>,
    /// The job's last exit code as `launchctl print` states it, which is a
    /// different field from [`Self::status`] and available in every domain.
    pub last_exit: Option<i64>,
    /// The variable names the unit file hands the program.
    pub env_keys: Vec<String>,
    /// Uppercase variables the program's own script reads.
    pub script_reads: Vec<String>,
    /// Uppercase variables that script sets or defaults for itself.
    pub script_assigns: Vec<String>,
    /// The argument vector the pid launchd holds is ACTUALLY executing, as the
    /// process table reports it, or empty when the label holds no pid.
    ///
    /// The declaration and the process are two different facts and this fleet
    /// has had them disagree: `com.wisent.compute.service.stado-local-control-plane`
    /// declares `stado coordinator` and launchd was holding a five-day-old
    /// `stado dashboard` under it — a command the product deleted on
    /// 2026-08-19, whose refresh loop still forced a disk-cleanup pass every
    /// two minutes and stamped the janitor's shared interval out from under
    /// the queue agent. Every report that read the unit file agreed with
    /// itself and none of them was looking at the process.
    pub running_program: String,
    /// When that process started, and when the binary it is executing was
    /// last written. A process older than its own binary is running code
    /// nobody shipped: the delivery landed, the unit was never restarted, and
    /// the label keeps answering with the previous version.
    pub started_epoch: Option<i64>,
    pub binary_written_epoch: Option<i64>,
}

impl UndeclaredUnit {
    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "label": self.label,
            "pid": self.pid,
            "status": self.status,
            "declared": self.declared,
            "path": self.path,
            "path_source": self.path_source,
            "classification": self.classification(),
            "program": self.program,
            "declaring_paths": self.declaring_paths,
            "running_program": self.running_program,
            "started_epoch": self.started_epoch,
            "binary_written_epoch": self.binary_written_epoch,
        })
    }

    /// Does this label carry the prefix every unit this fleet installs
    /// carries?
    ///
    /// A fact about the name, kept out of every path that decides what to
    /// enumerate. See [`FLEET_LABEL_PREFIX`].
    pub fn in_fleet_prefix(&self) -> bool {
        self.label.starts_with(FLEET_LABEL_PREFIX)
    }

    /// Is there evidence tying this label to this fleet, independent of what it
    /// is called?
    ///
    /// Two facts, both read off the host: its unit file sits in one of the
    /// three launchd directories this fleet installs into, or the program it is
    /// running executes out of a declared product root.
    ///
    /// This exists because the widened enumeration has to stay readable.
    /// charless-mac-mini loads 537 labels and 494 of them are `com.apple.*`;
    /// a report that prints all of them equally has buried its finding as
    /// effectively as the prefix filter did, and burying a finding in noise is
    /// the failure this whole change is about. So the noise is separated by
    /// EVIDENCE rather than by spelling: every one of the six rows that
    /// mattered on that host — `com.stado.agent.charless-mac-mini`, three
    /// `ai.wisent.oko.*` agents and two `actions.runner.*` runners — has its
    /// plist in `~/Library/LaunchAgents` or `/Library/LaunchDaemons`, and not
    /// one `application.com.apple.*` row does.
    pub fn fleet_affiliated(&self) -> bool {
        !self.declaring_paths.is_empty()
            || (!self.running_program.is_empty()
                && product_guess(&self.running_program) != UNKNOWN_PRODUCT)
    }

    /// The sentence a report should use about this row, once the registry has
    /// been asked whether it declares the label.
    ///
    /// `declared` is the registry's own unit. `undeclared` is a label the
    /// registry does not declare that is spelled like one of ours — a
    /// duplicate agent, a superseded convention, a unit somebody bootstrapped
    /// by hand. `outside-fleet-prefix` is a label the registry does not declare
    /// and did not name either, yet which this host ties to the fleet anyway:
    /// the class that used to be invisible, and the one the janitor's writer
    /// was in. `unaffiliated` is a loaded job with no tie to this fleet at all
    /// — the platform's own agents.
    ///
    /// All four are enumerated and counted. The class chooses the sentence and
    /// the order rows are printed in, never whether the host was asked.
    pub fn classification(&self) -> &'static str {
        match (
            self.declared,
            self.in_fleet_prefix(),
            self.fleet_affiliated(),
        ) {
            (true, _, _) => "declared",
            (false, true, _) => "undeclared",
            (false, false, true) => "outside-fleet-prefix",
            (false, false, false) => "unaffiliated",
        }
    }

    /// Is this a row an operator has to act on? `declared` is the answer the
    /// document promised; `unaffiliated` is somebody else's job on the same
    /// machine. What is left is what this fleet put there and cannot account
    /// for.
    pub fn accounted_for(&self) -> bool {
        self.declared || self.classification() == "unaffiliated"
    }

    /// The first word of an argument vector: the program, without its flags.
    fn head(vector: &str) -> Option<&str> {
        vector.split_whitespace().next()
    }

    /// `plutil -extract ... json` escapes every path separator, so a declared
    /// program arrives as `\/Users\/charles\/...`. Comparing that against a
    /// process table entry is comparing two spellings of the same path.
    fn unescape(vector: &str) -> String {
        vector.replace("\\/", "/")
    }

    /// The first argument after `binary` that is not a flag: the subcommand.
    ///
    /// Anchored on the binary rather than on argv[0], because an interpreter
    /// is a legitimate argv[0]: `python3 .../uvicorn app.main:app` declares
    /// `uvicorn` as its program and the process table shows the interpreter
    /// first. Reading position 1 there compares `app.main:app` against the
    /// path of uvicorn itself and calls three healthy services broken.
    fn subcommand<'a>(vector: &'a str, binary: &str) -> Option<&'a str> {
        let mut words = vector.split_whitespace().skip_while(|word| *word != binary);
        words.next()?;
        words.find(|word| !word.starts_with('-'))
    }

    /// Is the label's live process executing the program its own unit file
    /// declares?
    ///
    /// `None` wherever the answer would be a guess, which is most of a real
    /// host: a label with no pid, an unreadable unit file, and — deliberately
    /// — every case where the declared binary does not appear in the running
    /// argv at all. That last one is the launcher shape, and it is legitimate
    /// and everywhere: `launch-mac.sh` execs `node`, a `.venv/bin/uvicorn`
    /// declaration runs as `/opt/homebrew/.../Python`, `mac-mini-web-launch.sh`
    /// becomes `npm start`. An exec chain and a wrong program are
    /// indistinguishable from the outside, so this check says nothing there
    /// rather than reporting fourteen healthy services to bury one real
    /// finding.
    ///
    /// What it does answer is the one case where the evidence is complete: the
    /// process IS running the declared binary, and the subcommand differs.
    /// Every stado unit on a host executes the same binary, so the subcommand
    /// is the entire difference between the coordinator, the agent and a
    /// dashboard the product deleted in August.
    pub fn runs_declared_program(&self) -> Option<bool> {
        if self.running_program.is_empty() || self.program.is_empty() {
            return None;
        }
        let declared = Self::unescape(&self.program);
        let binary = Self::head(&declared)?;
        if !self
            .running_program
            .split_whitespace()
            .any(|word| word == binary)
        {
            return None;
        }
        match (
            Self::subcommand(&declared, binary),
            Self::subcommand(&self.running_program, binary),
        ) {
            (Some(declared_word), Some(running_word)) => Some(declared_word == running_word),
            (None, None) => Some(true),
            _ => None,
        }
    }

    /// Is it executing the binary that is on disk now? `None` when either
    /// timestamp is missing, for the same reason.
    pub fn runs_current_binary(&self) -> Option<bool> {
        let started = self.started_epoch?;
        let written = self.binary_written_epoch?;
        Some(written <= started)
    }

    /// The binary the live process is executing, for a report that has to name
    /// it.
    pub fn running_binary(&self) -> Option<&str> {
        Self::head(&self.running_program)
    }

    /// The program the unit file declares, in the spelling a process table
    /// uses, so a report can print the two side by side.
    pub fn declared_program(&self) -> String {
        Self::unescape(&self.program)
    }

    /// How long after this process started its binary was replaced, in
    /// seconds. `None` unless both facts were read.
    pub fn binary_written_after_start(&self) -> Option<i64> {
        Some(self.binary_written_epoch? - self.started_epoch?)
    }
}

/// Read-only: `launchctl list`, `launchctl print` for the three domains,
/// `launchctl dumpstate` once, and the unit files those name. It starts
/// nothing, stops nothing, signals nothing and needs no sudo.
///
/// Three reads, then one join, then one pass per label. The shape matters
/// because this script is what the fleet sweep spends its budget on. The
/// version before this one asked launchd again for every label -- five `awk`
/// passes over two in-memory tables and up to three `launchctl print
/// <domain>/<label>` calls each -- so a mac carrying 1,034 labels spawned
/// something near fifteen thousand processes to answer questions that one
/// pass answers for every label at once. It took longer than
/// [`crate::deploy::host_recovery::TIMEOUT_SECONDS`], the channel killed it,
/// and `stado doctor` reported `lukasz-macbook: not measured` -- the host
/// running the sweep was the one host the sweep could never finish. Measured
/// on that host: 28 seconds for 1,159 labels, against a 120-second cap it
/// used to exceed.
const LOADED_LABELS_SCRIPT: &str = r##"set -u
if [ "$(/usr/bin/uname -s)" != Darwin ]; then
  printf 'STADO_LOADED_UNSUPPORTED\t%s\n' "$(/usr/bin/uname -s)"
  exit 0
fi
uid=$(/usr/bin/id -u)
listing=$(/bin/launchctl list)

# Every job launchd actually HOLDS, in every domain this login can print.
#
# `launchctl list` prints one domain, and this script used to enumerate from it
# plus the three unit directories. A job loaded in the SYSTEM domain whose
# plist has been deleted is in neither half, so it was never a candidate. That
# is not a corner: on 2026-09-01 the label
# `com.wisent.compute.service.com.wisent.compute.service.stado-agent-mini` was
# loaded in the system domain with KeepAlive and no file on disk, and it
# recreated an undeclared `stado agent` on charless-mac-mini for days while
# `list --undeclared`, `list --unowned` and the reap keep-set each answered,
# for three different reasons, that no label held it.
holds=''
for domain in system "user/$uid" "gui/$uid"; do
  block=$(/bin/launchctl print "$domain" 2>/dev/null) || continue
  [ -n "$block" ] || continue
  rows=$(printf '%s\n' "$block" | /usr/bin/awk -v d="$domain" '
    /^[ \t]*services = \{/ { inside = 1; next }
    inside && /^[ \t]*\}/ { inside = 0 }
    inside {
      n = split($0, f, /[ \t]+/)
      while (n > 0 && f[n] == "") { n-- }
      if (n < 1) next
      lbl = f[n]
      if (lbl == "label" || lbl == "Label" || lbl == "PID") next
      p = (n >= 3 ? f[n - 2] : "")
      s = (n >= 2 ? f[n - 1] : "")
      printf "H\t%s\t%s\t%s\t%s\n", lbl, d, p, s
    }')
  holds="$holds
$rows"
done

# The unit file launchd loaded each job from, how many times it has started it,
# and how it last ended -- for every service in every domain, in one read.
#
# `launchctl dumpstate` states the whole world at once. The loop below used to
# ask `launchctl print <domain>/<label>` for these three columns, per label,
# per domain. The old comment here rejected dumpstate because it also carries
# every job's environment; that was an argument about what may LEAVE the host,
# and it is answered by extracting the three columns here rather than by
# spawning three thousand processes. Nothing but path, runs and last exit
# crosses the channel.
#
# A one-shot under KeepAlive reads `active` everywhere.
# `com.wisent.compute.service.com.wisent.claude-reauth-once` -- a job whose own
# name says `once` -- has run more than fifty thousand times, exiting 1 every
# time, into a log nobody read.
state=$(/bin/launchctl dumpstate 2>/dev/null | /usr/bin/awk '
  /^[^ \t].*= \{$/ { key = $1; next }
  /^\}/ { key = ""; next }
  key == "" { next }
  /^\tpath = / { sub(/^\tpath = /, ""); path[key] = $0; next }
  /^\truns = / { sub(/^\truns = /, ""); runs[key] = $0; next }
  /^\tlast exit code = / { sub(/^\tlast exit code = /, ""); exited[key] = $0; next }
  END {
    for (k in path) seen[k] = 1
    for (k in runs) seen[k] = 1
    for (k in exited) seen[k] = 1
    for (k in seen) {
      slash = 0
      for (i = length(k); i > 0; i--) { if (substr(k, i, 1) == "/") { slash = i; break } }
      if (slash < 2) continue
      printf "S\t%s\t%s\t%s\t%s\t%s\n", substr(k, slash + 1), substr(k, 1, slash - 1),
        (k in path ? path[k] : ""), (k in runs ? runs[k] : ""), (k in exited ? exited[k] : "")
    }
  }')

# Every label every source knows, joined once, in one `awk`.
joined=$(
  {
    printf '%s\n' "$listing" |
      /usr/bin/awk -F'\t' 'NF == 3 && $3 != "Label" { printf "L\t%s\t%s\t%s\n", $3, $1, $2 }'
    printf '%s\n' "$holds"
    printf '%s\n' "$state"
    for directory in /Library/LaunchDaemons "$HOME/Library/LaunchAgents" /Library/LaunchAgents; do
      for file in "$directory"/*.plist; do
        [ -f "$file" ] || continue
        base=${file##*/}
        printf 'F\t%s\t%s\n' "${base%.plist}" "$file"
      done
    done
  } | /usr/bin/awk -F'\t' '
    $1 == "L" { label[$2] = 1; lpid[$2] = $3; lstatus[$2] = $4; next }
    $1 == "H" {
      label[$2] = 1
      domains[$2] = domains[$2] $3 " "
      if (hpid[$2] == "") hpid[$2] = $4
      if (hstatus[$2] == "") hstatus[$2] = $5
      next
    }
    $1 == "S" {
      label[$2] = 1
      key = $2 SUBSEP $3
      spath[key] = $4; sruns[key] = $5; sexit[key] = $6
      order[$2] = order[$2] $3 " "
      next
    }
    $1 == "F" { label[$2] = 1; files[$2] = files[$2] $3 " "; next }
    END {
      for (l in label) {
        pid = lpid[l]; status = lstatus[l]
        # The domain table wins over the single-domain listing: it is the only
        # one of the two that can speak for the system domain.
        if (hpid[l] ~ /^[0-9]+$/) pid = hpid[l]
        if (status == "" && hstatus[l] ~ /^-?[0-9]+$/) status = hstatus[l]
        # A domain the job is loaded in answers first; any domain that knows
        # the label answers second. Both beat asking launchd again.
        chosen = ""
        n = split(domains[l], held, " ")
        for (i = 1; i <= n; i++) {
          if (held[i] != "" && (l SUBSEP held[i]) in spath) { chosen = held[i]; break }
        }
        if (chosen == "") {
          n = split(order[l], any, " ")
          for (i = 1; i <= n; i++) { if (any[i] != "") { chosen = any[i]; break } }
        }
        key = l SUBSEP chosen
        printf "J\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", l, pid, status, domains[l],
          (key in spath ? spath[key] : ""), (key in sruns ? sruns[key] : ""),
          (key in sexit ? sexit[key] : ""), files[l]
      }
    }'
)

printf '%s\n' "$joined" | while IFS="$(printf '\t')" read -r tag label pid status loaded_domains launchd_path runs exited unit_files; do
  [ "$tag" = J ] || continue
  [ -n "$label" ] || continue
  case "$pid" in ''|*[!0-9]*) pid='' ;; esac
  case "$runs" in ''|*[!0-9]*) runs='' ;; esac
  case "$exited" in ''|*[!0-9-]*) exited='' ;; esac
  plist=''
  source=''
  domains=''
  for candidate in "/Library/LaunchDaemons/$label.plist" "$HOME/Library/LaunchAgents/$label.plist" "/Library/LaunchAgents/$label.plist"; do
    if [ -f "$candidate" ]; then
      if [ -z "$plist" ]; then plist="$candidate"; source='fleet-directory'; fi
      domains="${domains:+$domains }$candidate"
    fi
  done
  # A loaded label whose file is in none of the three directories this fleet
  # installs into. launchd knows where it loaded the job from, and the join
  # above already carries that answer.
  #
  # Only an absolute path is accepted. For a label with no unit file at all
  # -- every `application.com.apple.*` row on a mac -- launchd answers
  # `path = (submitted by runningboardd.190)`, and reporting that in a
  # UNIT_FILE column is a sentence that reads like a location and is not one.
  if [ -z "$plist" ]; then
    case "$launchd_path" in
      /*) plist="$launchd_path"; source='launchd' ;;
    esac
  fi
  program=''
  env_keys=''
  if [ -n "$plist" ] && [ -r "$plist" ]; then
    program=$(/usr/bin/plutil -extract ProgramArguments json -o - "$plist" 2>/dev/null \
      | /usr/bin/tr -d '[]"' | /usr/bin/tr ',' ' ' | /usr/bin/tr '\t\r\n' ' ')
    # The variables the unit file hands its program. A launchd job inherits
    # almost nothing, so a plist that names none of what its program requires
    # is a unit that cannot work -- and it fails on its interval, quietly,
    # forever. The beacon relay on lukasz-macbook carried HOME and PATH while
    # its program required STADO_HOST_HEALTH_API_URL, and it failed every five
    # minutes for three weeks.
    env_keys=$(/usr/bin/plutil -extract EnvironmentVariables json -o - "$plist" 2>/dev/null \
      | /usr/bin/tr -d '{}"' | /usr/bin/tr ',' '\n' \
      | /usr/bin/awk -F':' 'NF > 1 { print $1 }' | /usr/bin/tr '\n' ' ')
  fi
  # Which uppercase variables the program's own script reads, and which it sets
  # or defaults for itself. The subtraction happens in the reader, against the
  # set launchd does provide, because a shell is the wrong place to hold that
  # policy.
  needs=''
  assigns=''
  # `plutil -extract ... json` renders every path separator escaped
  # (`\/Users\/charles\/...`), which every earlier reader undid in Rust. This
  # one has to OPEN the file, so it unescapes here or it opens nothing -- and
  # opening nothing is how this check measured zero subjects on its first run
  # while looking, from the outside, exactly like a host with no problem.
  unescaped=$(printf '%s' "$program" | /usr/bin/tr -d '\\')
  script=$(printf '%s' "$unescaped" | /usr/bin/awk '{ print $1 }')
  case "$script" in
    */bash|*/sh|*/zsh|*/env) script=$(printf '%s' "$unescaped" | /usr/bin/awk '{ print $2 }') ;;
  esac
  case "$script" in
    "$HOME"/*)
      if [ -f "$script" ] && [ -r "$script" ]; then
        # `-I`, because the guard above is a path prefix and a path prefix does
        # not make a file a script. The old comment here promised "a fleet
        # script, never a system binary" and nothing enforced it:
        # `$HOME/.stado/bin/skarbiec`, `$HOME/.stado/marketplace/bin/wisent-agent`
        # and `$HOME/Applications/Oko.app/Contents/MacOS/Oko` are Mach-O
        # binaries of tens of megabytes. Grepping them cost most of this
        # script's runtime and answered `Binary file ... matches`, which the
        # reader then printed as the list of variables the program reads. A
        # binary states nothing about its environment here, and stating
        # nothing is the honest answer for it.
        needs=$(/usr/bin/grep -I -o -E '[$][{]?[A-Z][A-Z0-9_]{2,}' "$script" 2>/dev/null \
          | /usr/bin/tr -d '${' | /usr/bin/sort -u | /usr/bin/tr '\n' ' ')
        assigns=$(/usr/bin/grep -I -o -E '(^|[ \t;])(export[ ]+)?[A-Z][A-Z0-9_]{2,}=|[$][{][A-Z][A-Z0-9_]{2,}:[-=?]' "$script" 2>/dev/null \
          | /usr/bin/tr -d '${}:-=?' | /usr/bin/sed 's/^[ \t;]*//; s/^export[ ]*//' \
          | /usr/bin/sort -u | /usr/bin/tr '\n' ' ')
      fi
      ;;
  esac
  running=''
  started=''
  written=''
  case "$pid" in
    ''|*[!0-9]*) ;;
    *)
      running=$(/bin/ps -p "$pid" -o command= 2>/dev/null | /usr/bin/tr '\t\r\n' ' ')
      lstart=$(/bin/ps -p "$pid" -o lstart= 2>/dev/null)
      started=$(/bin/date -j -f '%a %b %d %T %Y' "$lstart" +%s 2>/dev/null)
      binary=$(/bin/ps -p "$pid" -o comm= 2>/dev/null)
      if [ -f "$binary" ]; then written=$(/usr/bin/stat -f %m "$binary" 2>/dev/null); fi
      ;;
  esac
  printf 'STADO_LOADED\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$pid" "$status" "$label" "${plist:--}" "${program:--}" "${domains:--}" "${running:--}" "${started:--}" "${written:--}" "${source:--}" "${loaded_domains:--}" "${runs:--}" "${exited:--}" "${env_keys:--}" "${needs:--}" "${assigns:--}"
done
# Every place a shell on this host could find a `stado`, and which one the
# release channel delivered.
#
# `~/.cargo/bin/stado` at 0.7.34 shadowed a delivered 0.13.40 for a week, and
# 0.7.34 has no `--undeclared`, no `bootout` and no `reap`: every answer it
# gave was "this host is clean", not because the host was, but because that
# binary could not look.
#
# `command -v` alone is not the question. This program runs on the channel's
# non-interactive shell, whose PATH is not the login shell's -- on
# charless-mac-mini it resolved NOTHING, and the reader called that agreement
# with the delivered binary. So the concrete locations are probed by name, a
# stale copy in any of them is a finding, and a location that could not be read
# is reported as unread rather than as clean.
delivered="$HOME/.stado/bin/stado"
delivered_version=''
delivered_real=''
if [ -x "$delivered" ]; then
  delivered_version=$("$delivered" --version 2>/dev/null | /usr/bin/awk '{ print $2; exit }')
  delivered_real=$(/usr/bin/readlink -f "$delivered" 2>/dev/null || printf '%s' "$delivered")
fi
printf 'STADO_PATH_DELIVERED\t%s\t%s\t%s\n' "$delivered" "${delivered_version:--}" "${delivered_real:--}"
# The delivered path is itself a candidate. Without it a host that carries
# exactly one correct binary measured ZERO locations and the reader had nothing
# to compare -- honest, but useless, and indistinguishable from a host nobody
# looked at.
#
# `-L` as well as `-e`: a DANGLING symlink on a PATH directory is not nothing,
# it is a `stado` that a shell finds and cannot execute.
for candidate in "$delivered" "$(command -v stado 2>/dev/null || true)" "$HOME/.cargo/bin/stado" "$HOME/.local/bin/stado" /usr/local/bin/stado /opt/homebrew/bin/stado; do
  [ -n "$candidate" ] || continue
  if [ ! -e "$candidate" ] && [ ! -L "$candidate" ]; then continue; fi
  version=''
  real=$(/usr/bin/readlink -f "$candidate" 2>/dev/null || printf '%s' "$candidate")
  if [ -x "$candidate" ]; then
    version=$("$candidate" --version 2>/dev/null | /usr/bin/awk '{ print $2; exit }')
  fi
  printf 'STADO_PATH_CANDIDATE\t%s\t%s\t%s\n' "$candidate" "${version:--}" "${real:--}"
done
"##;

/// End every product process on TARGET that no DECLARED unit owns.
///
/// `service stop` ends a declared unit and the processes launchd disowned from
/// it. Nothing ended a product process whose label the registry never declared,
/// or whose label has since been removed while the process kept running — and
/// on charless-mac-mini that is how a `stado agent` from 2026-08-27 went on
/// publishing this host's capacity through three release deliveries, two
/// restarts, a `service stop` and a `service remove`, refusing 55 pinned jobs
/// the whole time.
///
/// Ownership here is the registry's, not launchd's. `unowned_processes` asks
/// whether ANY launchd job claims the process, and on a mac that set is about a
/// thousand pids, so a duplicate under an undeclared label reads as owned and
/// is left alone. This asks the question that matters: is this process the one
/// the document says should be running.
///
/// `SIGTERM` only, and only to processes executing out of a managed root whose
/// pid is not held by a declared label and is not a descendant of one. Nothing
/// is signalled on a `--dry-run`, which is the default at the CLI.
const REAP_SCRIPT: &str = "set -u
if [ \"$(/usr/bin/uname -s)\" != Darwin ]; then
  printf 'STADO_REAP_UNSUPPORTED\\t%s\\n' \"$(/usr/bin/uname -s)\"
  exit 0
fi
apply=@APPLY@
match=@MATCH@
set -- @ROOTS@
# The pids the DECLARED labels hold, and their descendants. Everything else
# under a managed root is a process the document does not account for.
#
# `launchctl list` prints only the domain this login can print, so a declared
# SYSTEM LaunchDaemon's pid was never in this set and every process it owns read
# as unowned. On charless-mac-mini on 2026-09-01 that made a fleet-wide
# `reap --command 'stado agent'` propose ending pid 3963 -- the queue agent
# `service ensure` had just installed as `com.wisent.compute.service.stado-agent-mini`
# in the system domain, thirty seconds earlier, and the only DECLARED agent the
# host had. Its argv is byte-identical to the undeclared duplicate beside it, so
# no `--command` substring could separate them and the operator's only options
# were to end the declared agent too or not to reap at all.
#
# So the keep-set asks launchd for the label when the listing does not have it.
# `launchctl print <domain>/<label>` reads the system domain without privilege
# and states the `pid` the job holds; only the `pid` line is taken. This is the
# same blindness the loaded-label scan had against `com.wisent.*` and the same
# remedy: ask the host about the whole world, not about the part one command
# happens to print.
keep=''
uid=$(/usr/bin/id -u)
listing=$(/bin/launchctl list)
for label in @LABELS@; do
  pid=$(printf '%s\\n' \"$listing\" | /usr/bin/awk -F'\\t' -v l=\"$label\" '$3 == l && $1 ~ /^[0-9]+$/ { print $1 }')
  if [ -z \"$pid\" ]; then
    for domain in system \"user/$uid\" \"gui/$uid\"; do
      pid=$(/bin/launchctl print \"$domain/$label\" 2>/dev/null |
        /usr/bin/awk -F' = ' '$1 ~ /^[[:space:]]*pid$/ { print $2; exit }' |
        /usr/bin/tr -d ' ')
      case \"$pid\" in
        ''|*[!0-9]*) pid='' ;;
        *) break ;;
      esac
    done
  fi
  if [ -n \"$pid\" ]; then keep=\"$keep $pid\"; fi
done
kept() {
  walk=\"$1\"
  while [ -n \"$walk\" ] && [ \"$walk\" != 0 ] && [ \"$walk\" != 1 ]; do
    case \" $keep \" in *\" $walk \"*) return 0 ;; esac
    walk=$(/bin/ps -p \"$walk\" -o ppid= 2>/dev/null | /usr/bin/tr -d ' ')
  done
  return 1
}
printf 'STADO_REAP_KEEP\\t%s\\n' \"$(printf '%s' \"$keep\" | /usr/bin/tr -s ' ')\"
self=$$
seen=''
for root in \"$@\"; do
  for pid in $(/usr/bin/pgrep -f \"$root\" 2>/dev/null); do
    case \" $seen \" in *\" $pid \"*) continue ;; esac
    if [ \"$pid\" = \"$self\" ]; then continue; fi
    command=$(/bin/ps -p \"$pid\" -o command= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    if [ -z \"$command\" ]; then continue; fi
    exe=$(/bin/ps -p \"$pid\" -o comm= 2>/dev/null)
    entry=$(printf '%s' \"$command\" | /usr/bin/awk '{ print $2 }')
    under=no
    case \"$exe\" in \"$root\"*) under=yes ;; esac
    case \"$entry\" in \"$root\"*) under=yes ;; esac
    if [ \"$under\" = no ]; then continue; fi
    # The operator names the exact program being de-duplicated. Without this
    # the keep-set decides the blast radius, and launchd holds a pid for only
    # some declared labels: a fleet-wide dry run on charless-mac-mini proposed
    # ending `skarbiec serve`, `stado dashboard`, `stado resolver serve` and the
    # Weles API server, every one of them a live service, because their pids are
    # not the ones their labels hold. One named program cannot do that.
    case \"$command\" in *\"$match\"*) ;; *) continue ;; esac
    seen=\"$seen $pid\"
    started=$(/bin/ps -p \"$pid\" -o lstart= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    # A kept pid is never signalled, and it used to be dropped here - before
    # its command was ever printed. That hid the one process an operator most
    # needs to name: the program a DECLARED label is holding, which is where a
    # stale binary survives a delivery. On charless-mac-mini the writer
    # starving the janitor's interval was pid 78635 under
    # `com.wisent.compute.service.stado-local-control-plane`, and every reap
    # report could say only its number. Reporting is not signalling: the row
    # reads `kept` and the loop still refuses to touch it.
    if kept \"$pid\"; then
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'kept' \"$started\" \"$command\"
      continue
    fi
    if [ \"$apply\" != yes ]; then
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'would_end' \"$started\" \"$command\"
      continue
    fi
    /bin/kill \"$pid\" 2>/dev/null || true
    /bin/sleep 2
    if /bin/ps -p \"$pid\" -o pid= >/dev/null 2>&1; then
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'still_running' \"$started\" \"$command\"
    else
      printf 'STADO_REAP\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" 'ended' \"$started\" \"$command\"
    fi
  done
done
";

/// A command substring that can ride inside double quotes in the fixed remote
/// program.
///
/// [`quote_unit_path`] refuses a space, which is right for a unit path and
/// wrong here: the whole point of the filter is to name
/// `stado agent --target <host>` rather than a bare binary. The charset is
/// widened by exactly a space and nothing else, so every character a shell
/// would act on stays refused. Shared with
/// [`super::service_spawn_watch`], which filters the same process table for
/// the same kind of name and must refuse exactly what the reaper refuses.
pub fn quote_command_match(value: &str) -> Result<String, DeployError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(DeployError("the command substring is empty".to_string()));
    }
    let allowed = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(c, ' ' | '-' | '_' | '.' | '/' | '=' | ':' | '+' | ',')
    };
    if let Some(bad) = trimmed.chars().find(|c| !allowed(*c)) {
        return Err(DeployError(format!(
            "command substring {trimmed:?} contains {bad:?}, which cannot ride the fixed remote \
             program"
        )));
    }
    Ok(trimmed.to_string())
}

/// Boot one exact init-system unit out of the system or calling-account scope.
///
/// The last gap in the world-to-declaration direction. `service list
/// --undeclared` can name a unit the registry never declared, and
/// `host remove-file` can delete its unit file — but `service stop` refuses a
/// unit with no declaration to resolve. A loaded unit whose file is already
/// gone otherwise has no owner left that can stop it.
///
/// On launchd, system jobs use the same `sudo -n` grant `ENSURE_BODY` uses.
/// User jobs are removed from both explicit `gui/<uid>` and `user/<uid>`
/// domains, and each domain is proven absent before success. On systemd,
/// system jobs use that same non-interactive privilege path and user jobs use
/// the calling account's explicit runtime bus. The exact requested unit is
/// disabled with `--now`; its identity, disabled state, and inactive state are
/// then read back before success. Neither branch removes a unit file.
///
/// `scope` exists because the system scope is tried first and returns. The same
/// name can identify distinct system and user jobs on either service manager,
/// so `User` is how an operator retires an undeclared duplicate without
/// touching its canonical system sibling. A unit the selected manager does not
/// hold is `absent`, not an error: repeated cleanup is safe.
const BOOTOUT_SCRIPT: &str = r#"set -u
label=@LABEL@
scope=@SCOPE@
report() { printf 'STADO_BOOTOUT\t%s\t%s\n' "$1" "$2"; }
os=$(/usr/bin/uname -s)
if [ "$os" = Darwin ]; then
  if [ "$scope" != user ] \
    && /usr/bin/sudo -n /bin/launchctl print "system/$label" >/dev/null 2>&1; then
    if ! /usr/bin/sudo -n /bin/launchctl bootout "system/$label" 2>/dev/null; then
      report refused "sudo -n launchctl bootout system/$label was refused"
      exit 0
    fi
    attempts=0
    while /usr/bin/sudo -n /bin/launchctl print "system/$label" >/dev/null 2>&1; do
      attempts=$((attempts + 1))
      if [ "$attempts" -ge 150 ]; then
        report refused "system/$label remained loaded after bootout"
        exit 0
      fi
      /bin/sleep 0.1
    done
    report booted_out "system/$label"
    exit 0
  fi
  uid=$(/usr/bin/id -u)
  removed=
  for domain in "gui/$uid" "user/$uid"; do
    if [ "$scope" = system ]; then continue; fi
    if /bin/launchctl print "$domain/$label" >/dev/null 2>&1; then
      if ! /bin/launchctl bootout "$domain/$label" 2>/dev/null; then
        report refused "launchctl bootout $domain/$label was refused"
        exit 0
      fi
      attempts=0
      while /bin/launchctl print "$domain/$label" >/dev/null 2>&1; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 150 ]; then
          report refused "$domain/$label remained loaded after bootout"
          exit 0
        fi
        /bin/sleep 0.1
      done
      removed="$removed $domain/$label"
    fi
  done
  if [ -n "$removed" ]; then
    report booted_out "${removed# }"
  else
    report absent "launchd holds no $scope job for $label (system, gui/$uid and user/$uid all read empty)"
  fi
  exit 0
fi
if [ "$os" != Linux ]; then
  report refused "unsupported service manager on $os"
  exit 0
fi

uid=$(/usr/bin/id -u)
if [ -x /usr/bin/sudo ]; then sudo_bin=/usr/bin/sudo; else sudo_bin=/bin/sudo; fi
systemd_refuse() {
  refusal=$(printf '%s' "$2" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-300)
  if [ -n "$refusal" ]; then
    report refused "$1: $refusal"
  else
    report refused "$1"
  fi
  exit 0
}
systemdctl() {
  manager_scope=$1
  shift
  if [ "$manager_scope" = system ]; then
    if [ "$uid" = 0 ]; then
      /usr/bin/systemctl "$@"
    else
      "$sudo_bin" -n /usr/bin/systemctl "$@"
    fi
    return
  fi
  runtime="/run/user/$uid"
  /usr/bin/env \
    XDG_RUNTIME_DIR="$runtime" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
    /usr/bin/systemctl --user "$@"
}
systemd_probe() {
  probe_scope=$1
  probe_output=$(systemdctl "$probe_scope" show --property=Id --property=LoadState -- "$label" 2>&1)
  probe_rc=$?
  if [ "$probe_rc" -ne 0 ]; then
    systemd_refuse "cannot inspect systemd $probe_scope/$label (status $probe_rc)" "$probe_output"
  fi
  probe_load=$(printf '%s\n' "$probe_output" | /usr/bin/awk -F= '$1 == "LoadState" { print $2; exit }')
  probe_id=$(printf '%s\n' "$probe_output" | /usr/bin/awk -F= '$1 == "Id" { sub(/^[^=]*=/, ""); print; exit }')
  case "$probe_load" in
    not-found) return 1 ;;
    loaded|masked) ;;
    error|bad-setting)
      systemd_refuse "systemd $probe_scope/$label has unusable load state $probe_load" "$probe_output"
      ;;
    *)
      systemd_refuse "systemd $probe_scope/$label returned unexpected load state ${probe_load:-empty}" "$probe_output"
      ;;
  esac
  if [ "$probe_id" != "$label" ]; then
    systemd_refuse "systemd $probe_scope name $label resolves to ${probe_id:-no unit id}; refusing a non-exact unit" ""
  fi
  return 0
}

selected=
if [ "$scope" != user ] && systemd_probe system; then selected=system; fi
if [ -z "$selected" ] && [ "$scope" != system ] && systemd_probe user; then selected=user; fi
if [ -z "$selected" ]; then
  case "$scope" in
    system|user) report absent "systemd $scope manager holds no exact unit named $label" ;;
    *) report absent "systemd system and user managers hold no exact unit named $label" ;;
  esac
  exit 0
fi

disable_output=$(systemdctl "$selected" disable --now -- "$label" 2>&1)
disable_rc=$?
if [ "$disable_rc" -ne 0 ]; then
  systemd_refuse "systemctl $selected disable --now $label failed (status $disable_rc)" "$disable_output"
fi

attempts=0
while :; do
  state_output=$(systemdctl "$selected" show --property=Id --property=LoadState --property=ActiveState -- "$label" 2>&1)
  state_rc=$?
  if [ "$state_rc" -ne 0 ]; then
    systemd_refuse "cannot verify systemd $selected/$label after disable (status $state_rc)" "$state_output"
  fi
  load_state=$(printf '%s\n' "$state_output" | /usr/bin/awk -F= '$1 == "LoadState" { print $2; exit }')
  active_state=$(printf '%s\n' "$state_output" | /usr/bin/awk -F= '$1 == "ActiveState" { print $2; exit }')
  state_id=$(printf '%s\n' "$state_output" | /usr/bin/awk -F= '$1 == "Id" { sub(/^[^=]*=/, ""); print; exit }')
  if [ "$load_state" = not-found ]; then
    active_state=not-found
    break
  fi
  if [ "$state_id" != "$label" ]; then
    systemd_refuse "systemd $selected/$label changed identity to ${state_id:-no unit id} during verification" "$state_output"
  fi
  if [ "$active_state" = inactive ]; then break; fi
  attempts=$((attempts + 1))
  if [ "$attempts" -ge 150 ]; then
    systemd_refuse "systemd $selected/$label remained ${active_state:-unknown} after disable --now" "$state_output"
  fi
  /bin/sleep 0.1
done

enabled_output=$(systemdctl "$selected" is-enabled -- "$label" 2>&1)
enabled_rc=$?
enabled_state=$(printf '%s\n' "$enabled_output" | /usr/bin/awk 'NF { state=$0 } END { print state }')
case "$enabled_state" in
  disabled|static|indirect|generated|transient|masked|masked-runtime|not-found) ;;
  enabled|enabled-runtime|linked|linked-runtime|alias)
    systemd_refuse "systemd $selected/$label remained enabled after disable --now" "$enabled_output"
    ;;
  *)
    systemd_refuse "cannot verify that systemd $selected/$label is disabled (status $enabled_rc)" "$enabled_output"
    ;;
esac
report booted_out "systemd $selected/$label is inactive and not enabled ($enabled_state)"
"#;

/// Which init-system scope a bootout may act in.
///
/// `Any` preserves the historical behaviour on both supported service
/// managers: system first, and the calling account only when the system scope
/// holds no exact unit by that name. The explicit variants distinguish names
/// that exist in both scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootoutScope {
    Any,
    System,
    User,
}

impl BootoutScope {
    /// The word the remote program compares against.
    pub fn word(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::System => "system",
            Self::User => "user",
        }
    }

    /// Parse an operator's `--domain`. `None` is [`Self::Any`].
    pub fn parse(value: Option<&str>) -> Result<Self, DeployError> {
        match value.map(str::trim) {
            None | Some("") | Some("any") => Ok(Self::Any),
            Some("system") => Ok(Self::System),
            Some("user") => Ok(Self::User),
            Some(other) => Err(DeployError(format!(
                "{other:?} is not an init-system scope: system, user, or any"
            ))),
        }
    }
}

/// Run [`BOOTOUT_SCRIPT`] for one exact unit name. Returns `(state, detail)`.
pub async fn bootout_label(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<(String, String), DeployError> {
    validate_unit_id(label)?;
    if label.contains('/') {
        return Err(DeployError(format!(
            "unit {} is not one exact launchd label or systemd unit name",
            py_str_repr(label)
        )));
    }
    let script = BOOTOUT_SCRIPT
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?))
        .replace("@SCOPE@", scope.word());
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the bootout did not complete",
        )));
    }
    output
        .stdout
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_BOOTOUT", state, detail] => {
                Some(((*state).trim().to_string(), (*detail).trim().to_string()))
            }
            _ => None,
        })
        .ok_or_else(|| DeployError(format!("{}: the bootout reported nothing", target.name)))
}
const AUTOSTART_SCRIPT: &str = r#"set -u
label=@LABEL@
action=@ACTION@
requested_scope=@REQUESTED_SCOPE@
report() { printf 'STADO_AUTOSTART\t%s\t%s\n' "$1" "$2"; }
os=$(/usr/bin/uname -s)
if [ "$os" = Darwin ]; then
  uid=$(/usr/bin/id -u)
  launch=/bin/launchctl
  disabled_state() {
    state_scope=$1
    if [ "$state_scope" = system ]; then
      raw=$(/usr/bin/sudo -n "$launch" print-disabled system 2>/dev/null) || return 1
    else
      raw=$("$launch" print-disabled "$state_scope" 2>/dev/null) || return 1
    fi
    printf '%s\n' "$raw" | /usr/bin/awk -v wanted="\"$label\"" '
      $1 == wanted && $2 == "=>" {
        value=$3; gsub(/[;,]/, "", value); print value; found=1; exit
      }
      END { if (!found) print "false" }'
  }
  present_scope() {
    candidate=$1
    case "$candidate" in
      system)
        [ -e "/Library/LaunchDaemons/$label.plist" ] ||
          /usr/bin/sudo -n "$launch" print "system/$label" >/dev/null 2>&1
        ;;
      gui/*|user/*)
        [ -e "$HOME/Library/LaunchAgents/$label.plist" ] ||
          [ -e "/Library/LaunchAgents/$label.plist" ] ||
          "$launch" print "$candidate/$label" >/dev/null 2>&1
        ;;
      *) return 1 ;;
    esac
  }
  if [ "$requested_scope" = any ]; then
    scopes="system gui/$uid user/$uid"
  else
    scopes=$requested_scope
  fi
  found=no
  for candidate in $scopes; do
    present_scope "$candidate" || continue
    found=yes
    if [ "$action" != inspect ]; then
      if [ "$candidate" = system ]; then
        detail=$(/usr/bin/sudo -n "$launch" "$action" "$candidate/$label" 2>&1)
      else
        detail=$("$launch" "$action" "$candidate/$label" 2>&1)
      fi
      rc=$?
      if [ "$rc" -ne 0 ]; then
        report refused "$candidate status=$rc $detail"
        exit 0
      fi
    fi
    disabled=$(disabled_state "$candidate") || {
      report refused "$candidate print-disabled failed"
      exit 0
    }
    case "$disabled" in
      true|disabled) state=disabled ;;
      false|enabled) state=enabled ;;
      *) report refused "$candidate returned invalid disabled state $disabled"; exit 0 ;;
    esac
    if { [ "$action" = enable ] && [ "$state" != enabled ]; } ||
       { [ "$action" = disable ] && [ "$state" != disabled ]; }; then
      report refused "$candidate did not reach requested $action state"
      exit 0
    fi
    report "$candidate" "$state"
  done
  if [ "$found" = no ]; then report absent "$requested_scope"; fi
  exit 0
fi
if [ "$os" != Linux ]; then
  report refused "unsupported service manager on $os"
  exit 0
fi
uid=$(/usr/bin/id -u)
systemdctl() {
  manager_scope=$1
  shift
  if [ "$manager_scope" = system ]; then
    if [ "$uid" = 0 ]; then /usr/bin/systemctl "$@"; else /usr/bin/sudo -n /usr/bin/systemctl "$@"; fi
  else
    runtime="/run/user/$uid"
    /usr/bin/env XDG_RUNTIME_DIR="$runtime" \
      DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
      /usr/bin/systemctl --user "$@"
  fi
}
if [ "$requested_scope" = any ]; then scopes="system user"; else scopes=$requested_scope; fi
found=no
for candidate in $scopes; do
  load=$(systemdctl "$candidate" show --property=LoadState --value -- "$label" 2>/dev/null) || continue
  [ "$load" != not-found ] || continue
  found=yes
  if [ "$action" != inspect ]; then
    detail=$(systemdctl "$candidate" "$action" -- "$label" 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ]; then report refused "$candidate status=$rc $detail"; exit 0; fi
  fi
  enabled=$(systemdctl "$candidate" is-enabled -- "$label" 2>/dev/null || true)
  case "$enabled" in
    enabled|enabled-runtime|linked|linked-runtime|alias) state=enabled ;;
    disabled|static|indirect|generated|transient|masked|masked-runtime) state=disabled ;;
    *) report refused "$candidate returned invalid enabled state ${enabled:-empty}"; exit 0 ;;
  esac
  if { [ "$action" = enable ] && [ "$state" != enabled ]; } ||
     { [ "$action" = disable ] && [ "$state" != disabled ]; }; then
    report refused "$candidate did not reach requested $action state"
    exit 0
  fi
  report "$candidate" "$state"
done
if [ "$found" = no ]; then report absent "$requested_scope"; fi
"#;

fn autostart_script(label: &str, action: &str, scope: &str) -> Result<String, DeployError> {
    validate_unit_id(label)?;
    if !matches!(action, "inspect" | "enable" | "disable") {
        return Err(DeployError(format!("invalid autostart action {action:?}")));
    }
    let valid_scope = matches!(scope, "any" | "system" | "user")
        || scope
            .strip_prefix("gui/")
            .or_else(|| scope.strip_prefix("user/"))
            .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit()));
    if !valid_scope {
        return Err(DeployError(format!("invalid autostart scope {scope:?}")));
    }
    Ok(AUTOSTART_SCRIPT
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?))
        .replace("@ACTION@", action)
        .replace("@REQUESTED_SCOPE@", scope))
}

/// Read every installed/loaded init-system scope's persistent boot state for
/// one exact unit. `true` means enabled after reboot.
pub async fn label_autostart(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<BTreeMap<String, bool>, DeployError> {
    let output =
        host_channel::run_script(target, &autostart_script(label, "inspect", "any")?, runner)
            .await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "autostart inspection failed",
        )));
    }
    let mut states = BTreeMap::new();
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_AUTOSTART", "refused", detail] => {
                return Err(DeployError((*detail).to_string()));
            }
            ["STADO_AUTOSTART", scope, "enabled"] => {
                states.insert((*scope).to_string(), true);
            }
            ["STADO_AUTOSTART", scope, "disabled"] => {
                states.insert((*scope).to_string(), false);
            }
            _ => {}
        }
    }
    Ok(states)
}

/// Persist one exact init-system scope's boot state and verify the manager
/// reports the requested state.
pub async fn set_label_autostart(
    target: &ComputeTarget,
    label: &str,
    scope: &str,
    enabled: bool,
    runner: &Runner,
) -> Result<(), DeployError> {
    let action = if enabled { "enable" } else { "disable" };
    let output =
        host_channel::run_script(target, &autostart_script(label, action, scope)?, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "autostart mutation failed",
        )));
    }
    let expected = if enabled { "enabled" } else { "disabled" };
    let found = output.stdout.lines().any(|line| {
        matches!(
            host_channel::marker_fields(line).as_slice(),
            ["STADO_AUTOSTART", found_scope, found_state]
                if *found_scope == scope && *found_state == expected
        )
    });
    if !found {
        return Err(DeployError(format!(
            "{label} {scope} did not verify as {expected}"
        )));
    }
    Ok(())
}

const USER_LAUNCHAGENT_SCRIPT: &str = r#"set -u
label=@LABEL@
action=@ACTION@
path="$HOME/Library/LaunchAgents/$label.plist"
uid=$(/usr/bin/id -u)
gui="gui/$uid"
user="user/$uid"
report() { printf 'STADO_USER_LAUNCHAGENT\t%s\t%s\n' "$1" "$2"; }
if [ "$(/usr/bin/uname -s)" != Darwin ]; then
  report refused "not a launchd host"
  exit 0
fi
if [ "$action" = inspect ] && [ ! -e "$path" ] && [ ! -L "$path" ]; then
  if /bin/launchctl print "$gui/$label" >/dev/null 2>&1 ||
     /bin/launchctl print "$user/$label" >/dev/null 2>&1; then
    report refused "$label is loaded without a restorable LaunchAgent plist"
  else
    report absent "$path"
  fi
  exit 0
fi
if [ "$action" != delete ]; then
  if [ ! -f "$path" ] || [ -L "$path" ]; then
    report refused "$path is not a regular non-symlink LaunchAgent plist"
    exit 0
  fi
  if ! /usr/bin/plutil -lint "$path" >/dev/null 2>&1; then
    report refused "$path is not a valid plist"
    exit 0
  fi
fi
case "$action" in
  check)
    report ready "$path"
    ;;
  inspect)
    report ready "$path"
    ;;
  restore)
    if /bin/launchctl print "$gui/$label" >/dev/null 2>&1 ||
       /bin/launchctl print "$user/$label" >/dev/null 2>&1; then
      report already_loaded "$label is already loaded"
      exit 0
    fi
    domain="$user"
    if /bin/launchctl print "$gui" >/dev/null 2>&1; then domain="$gui"; fi
    failure=$(/bin/launchctl bootstrap "$domain" "$path" 2>&1)
    code=$?
    if [ "$code" -eq 0 ] &&
       /bin/launchctl print "$domain/$label" >/dev/null 2>&1; then
      report restored "$domain/$label"
    else
      failure=$(printf '%s' "$failure" | /usr/bin/tr '\t\r\n' '   ')
      report failed "launchctl bootstrap $domain exited $code: $failure"
    fi
    ;;
  delete)
    if /bin/launchctl print "$gui/$label" >/dev/null 2>&1 ||
       /bin/launchctl print "$user/$label" >/dev/null 2>&1; then
      report refused "$label remains loaded; its plist was not removed"
      exit 0
    fi
    if [ ! -e "$path" ] && [ ! -L "$path" ]; then
      report absent "$path"
      exit 0
    fi
    if [ ! -f "$path" ] || [ -L "$path" ]; then
      report refused "$path is not a regular non-symlink LaunchAgent plist"
      exit 0
    fi
    if /bin/rm -f "$path" && [ ! -e "$path" ] && [ ! -L "$path" ]; then
      report removed "$path"
    else
      report failed "could not remove $path"
    fi
    ;;
  *)
    report refused "unsupported internal action"
    ;;
esac
"#;

async fn user_launchagent_action(
    target: &ComputeTarget,
    label: &str,
    action: &str,
    runner: &Runner,
) -> Result<(String, String), DeployError> {
    validate_unit_id(label)?;
    if !matches!(action, "check" | "inspect" | "restore" | "delete") {
        return Err(DeployError(format!(
            "unsupported internal LaunchAgent action {action:?}"
        )));
    }
    let script = USER_LAUNCHAGENT_SCRIPT
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?))
        .replace("@ACTION@", &format!("\"{}\"", action));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the LaunchAgent action did not complete",
        )));
    }
    output
        .stdout
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_USER_LAUNCHAGENT", state, detail] => {
                Some(((*state).trim().to_string(), (*detail).trim().to_string()))
            }
            _ => None,
        })
        .ok_or_else(|| {
            DeployError(format!(
                "{}: the LaunchAgent action reported nothing",
                target.name
            ))
        })
}

/// Prove that a legacy user LaunchAgent can be restored before taking it down.
pub async fn check_user_launchagent(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let (state, detail) = user_launchagent_action(target, label, "check", runner).await?;
    if state == "ready" {
        Ok(())
    } else {
        Err(DeployError(format!(
            "cannot supersede user LaunchAgent {label}: {detail}"
        )))
    }
}

/// Whether a same-named user LaunchAgent exists and can be restored on rollback.
pub async fn restorable_user_launchagent_exists(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<bool, DeployError> {
    let (state, detail) = user_launchagent_action(target, label, "inspect", runner).await?;
    match state.as_str() {
        "ready" => Ok(true),
        "absent" => Ok(false),
        _ => Err(DeployError(format!(
            "cannot supersede user LaunchAgent {label}: {detail}"
        ))),
    }
}

/// Restore a user LaunchAgent whose replacement failed readiness.
pub async fn restore_user_launchagent(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let (state, detail) = user_launchagent_action(target, label, "restore", runner).await?;
    if state == "restored" || state == "already_loaded" {
        Ok(())
    } else {
        Err(DeployError(format!(
            "could not restore user LaunchAgent {label}: {detail}"
        )))
    }
}

/// Delete an unloaded superseded user LaunchAgent's fixed plist path.
pub async fn delete_user_launchagent(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let (state, detail) = user_launchagent_action(target, label, "delete", runner).await?;
    if state == "removed" || state == "absent" {
        Ok(())
    } else {
        Err(DeployError(format!(
            "could not delete superseded user LaunchAgent {label}: {detail}"
        )))
    }
}

/// One process the reaper judged, and what it did about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapedProcess {
    pub host: String,
    pub pid: String,
    /// `would_end`, `ended`, `still_running`, or `kept`.
    ///
    /// `kept` is a row the reaper refuses to signal because a declared label
    /// holds that pid or one of its ancestors. It is reported for the same
    /// reason the others are: naming what a declared label is actually
    /// running is how a stale binary that survived a delivery becomes
    /// visible, and the keep-set is exactly where such a process hides.
    pub outcome: String,
    pub started_at: String,
    pub command: String,
}

impl ReapedProcess {
    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "pid": self.pid,
            "outcome": self.outcome,
            "started_at": self.started_at,
            "command": self.command,
        })
    }
}

/// [`REAP_SCRIPT`] against one host. `apply` false signals nothing.
pub async fn reap_undeclared_processes(
    target: &ComputeTarget,
    command_match: &str,
    apply: bool,
    runner: &Runner,
) -> Result<(Vec<ReapedProcess>, String), DeployError> {
    if command_match.trim().is_empty() {
        return Err(DeployError(
            "a command substring is required: the reaper de-duplicates one named program, never \
             everything under a managed root"
                .to_string(),
        ));
    }
    let mut roots = Vec::new();
    for root in managed_roots()? {
        roots.push(format!("\"{}\"", quote_unit_path(&root)?));
    }
    let mut labels = Vec::new();
    for service in declared_services(target) {
        labels.push(format!("\"{}\"", quote_unit_path(service.unit_id())?));
    }
    let script = REAP_SCRIPT
        .replace("@ROOTS@", &roots.join(" "))
        .replace("@LABELS@", &labels.join(" "))
        .replace("@APPLY@", if apply { "yes" } else { "no" })
        .replace(
            "@MATCH@",
            &format!("\"{}\"", quote_command_match(command_match)?),
        );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the reap did not complete",
        )));
    }
    let mut kept = String::new();
    let mut reaped = Vec::new();
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_REAP_KEEP", pids] => kept = (*pids).trim().to_string(),
            ["STADO_REAP", pid, outcome, started, command] => reaped.push(ReapedProcess {
                host: target.name.clone(),
                pid: (*pid).trim().to_string(),
                outcome: (*outcome).trim().to_string(),
                started_at: (*started).trim().to_string(),
                command: (*command).trim().to_string(),
            }),
            _ => {}
        }
    }
    Ok((reaped, kept))
}

/// Every launchd job loaded on TARGET that the registry does not declare, with
/// the unit file and program each one runs.
///
/// This is the direction nothing in the product looked. `service list` walks
/// the declaration and asks the host about each entry; `list --unowned` walks
/// the processes and asks launchd who owns them — and on the always-on mac it
/// correctly answered that nothing is unowned, because every duplicate IS
/// owned, by a label the registry never heard of.
///
/// charless-mac-mini was running three queue agents at once under that blind
/// spot: `com.wisent.compute.service.stado-agent-mini`, the only one the
/// registry declares, plus `com.wisent.compute.agent.charless-mac-mini` from
/// `stado bootstrap --local`'s label convention and
/// `com.wisent.compute.service.stado-queue-agent` from a third. All three
/// published capacity for the same consumer id, so the oldest binary on the
/// box decided what the host answered, and 55 pinned jobs were refused for
/// seven days by a process no report could name.
///
/// Scope is the registry's declaration and nothing else. This used to be
/// "every job under [`FLEET_LABEL_PREFIX`] that the registry does not declare",
/// and the prefix was the entire hiding place: `com.stado.agent.charless-mac-mini`
/// was loaded on that same host on 2026-09-01, was the only label on it outside
/// the prefix, held the pid overwriting the janitor's state file — and this
/// function answered that the host had no undeclared unit. Callers that want to
/// treat an out-of-prefix row differently read
/// [`UndeclaredUnit::classification`]; nothing decides that by filtering.
pub async fn undeclared_units(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<UndeclaredUnit>, DeployError> {
    Ok(loaded_units(target, runner)
        .await?
        .into_iter()
        .filter(|unit| !unit.declared)
        .collect())
}

/// EVERY launchd label loaded on TARGET, plus every label with a unit file in
/// the three directories this fleet installs into — declared or not, fleet-named
/// or not, with every domain that declares it and the registry's verdict on each.
///
/// [`undeclared_units`] is this list minus the rows the registry declares, and
/// that subtraction is why one class of duplicate hid for a whole evening: a
/// label declared once as a system LaunchDaemon and once as a user LaunchAgent
/// is DECLARED, so it never appears in the undeclared view, while launchd runs
/// both copies. Three processes served one declared port on the always-on mac
/// behind exactly that. Callers that need to reason about duplication read this
/// one; callers that need to reason about ownership read the other.
///
/// Neither list is filtered by label any more. This function used to drop every
/// row outside [`FLEET_LABEL_PREFIX`] before returning, so the sweep and the
/// undeclared view were both blind to the same set, and the one job on
/// charless-mac-mini that mattered on 2026-09-01 was in it. A check that wants
/// a narrower population states that narrowing itself, from evidence it holds,
/// and says so where an operator can read it.
pub async fn loaded_units(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<UndeclaredUnit>, DeployError> {
    Ok(loaded_units_with_posture(target, runner).await?.0)
}

/// One `stado` a shell on the host could find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StadoCopy {
    /// The location probed.
    pub path: String,
    /// The version it reports, empty when it could not be run.
    pub version: String,
    /// `path` with every symlink followed.
    pub real: String,
}

/// Which `stado` binaries one host carries, and which one was delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBinary {
    /// The path the release channel delivers to.
    pub delivered: String,
    pub delivered_version: String,
    /// `delivered` with every symlink followed, empty when it is not there.
    pub delivered_real: String,
    /// Every location probed that exists.
    pub candidates: Vec<StadoCopy>,
}

impl PathBinary {
    /// The copies that are NOT the delivered binary.
    ///
    /// Compared by resolved real path, so a symlink from `~/.local/bin/stado`
    /// to the delivered file is the same binary and not a finding. A version
    /// match alone would not do: two builds of one version are not one file.
    pub fn shadows(&self) -> Vec<&StadoCopy> {
        if self.delivered_real.is_empty() {
            return Vec::new();
        }
        self.candidates
            .iter()
            .filter(|copy| !copy.real.is_empty() && copy.real != self.delivered_real)
            .collect()
    }

    /// Could this be judged at all?
    ///
    /// A host whose delivered binary could not be read, or where no location
    /// answered, is UNMEASURED and must not be reported as agreeing. The first
    /// version of this check called an empty answer clean, which is the
    /// false-negative shape this whole module exists to refuse.
    pub fn measurable(&self) -> bool {
        !self.delivered_real.is_empty() && !self.candidates.is_empty()
    }
}

/// [`LOADED_LABELS_SCRIPT`] once, for both of the things it answers: the
/// loaded units, and which `stado` the host's own PATH resolves.
///
/// One read, because both facts come out of one script and a sweep that asked
/// twice would pay two SSH round trips for one question.
pub async fn loaded_units_with_posture(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<(Vec<UndeclaredUnit>, Option<PathBinary>), DeployError> {
    let output = host_channel::run_script(target, LOADED_LABELS_SCRIPT, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the loaded-unit scan did not complete",
        )));
    }
    let declared: std::collections::BTreeSet<String> = declared_services(target)
        .iter()
        .map(|service| service.unit_id().to_string())
        .collect();
    let units: Vec<UndeclaredUnit> = output
        .stdout
        .lines()
        .filter_map(|line| match host_channel::marker_fields(line).as_slice() {
            [
                "STADO_LOADED",
                pid,
                status,
                label,
                path,
                program,
                domains,
                running,
                started,
                written,
                path_source,
                loaded_domains,
                runs,
                last_exit,
                env_keys,
                script_reads,
                script_assigns,
            ] => {
                let label = (*label).trim().to_string();
                Some(UndeclaredUnit {
                    host: target.name.clone(),
                    declared: declared.contains(&label),
                    label,
                    pid: (*pid).trim().trim_matches('-').to_string(),
                    status: (*status).trim().to_string(),
                    path: (*path).trim().trim_matches('-').to_string(),
                    path_source: (*path_source).trim().trim_matches('-').to_string(),
                    program: (*program).trim().to_string(),
                    declaring_paths: (*domains)
                        .split_whitespace()
                        .filter(|path| *path != "-")
                        .map(str::to_string)
                        .collect(),
                    loaded_domains: split_marker_list(loaded_domains),
                    runs: (*runs).trim().trim_matches('-').parse().ok(),
                    last_exit: (*last_exit).trim().trim_matches('-').parse().ok(),
                    env_keys: split_marker_list(env_keys),
                    script_reads: split_marker_list(script_reads),
                    script_assigns: split_marker_list(script_assigns),
                    running_program: (*running).trim().trim_matches('-').trim().to_string(),
                    started_epoch: started.trim().parse().ok(),
                    binary_written_epoch: written.trim().parse().ok(),
                })
            }
            _ => None,
        })
        .collect();
    let mut posture: Option<PathBinary> = None;
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_PATH_DELIVERED", delivered, version, real] => {
                posture = Some(PathBinary {
                    delivered: undash(delivered),
                    delivered_version: undash(version),
                    delivered_real: undash(real),
                    candidates: Vec::new(),
                });
            }
            ["STADO_PATH_CANDIDATE", path, version, real] => {
                if let Some(posture) = posture.as_mut() {
                    let copy = StadoCopy {
                        path: undash(path),
                        version: undash(version),
                        real: undash(real),
                    };
                    // `command -v` and an explicit location can name the same
                    // file; one copy is one finding, not two.
                    if !posture
                        .candidates
                        .iter()
                        .any(|seen| seen.real == copy.real && !copy.real.is_empty())
                    {
                        posture.candidates.push(copy);
                    }
                }
            }
            _ => {}
        }
    }
    Ok((units, posture))
}

/// One marker field, with the `-` a shell writes for "empty" removed.
fn undash(field: &str) -> String {
    let trimmed = field.trim();
    if trimmed == "-" {
        return String::new();
    }
    trimmed.to_string()
}

/// Split one whitespace-delimited marker field into its entries, dropping the
/// `-` a shell writes for "empty".
///
/// The remote programs in this module cannot emit an empty field — a bare
/// empty string between two tabs is indistinguishable from a lost column — so
/// they write `-` and every reader has to undo it. Doing that in one place
/// keeps six call sites from each getting it slightly wrong.
fn split_marker_list(field: &str) -> Vec<String> {
    field
        .split_whitespace()
        .filter(|entry| *entry != "-")
        .map(str::to_string)
        .collect()
}

/// The managed-service record a completed deploy or adopt should be
/// recorded under, built from what the host actually reported rather than
/// from what the operator hoped: the resolved unit id, the resolved path,
/// and the init system that answered.
pub fn record_from_report(
    host: &str,
    host_heuristic: Option<&str>,
    name: &str,
    report: &RemoteReport,
    managed_since: &str,
) -> ManagedService {
    let mut service = if report.kind() == KIND_LAUNCHD {
        launchd_service(
            host,
            &report.unit,
            &report.path,
            SOURCE_REGISTRY,
            managed_since,
        )
    } else {
        systemd_service(
            host,
            &report.unit,
            &report.path,
            SOURCE_REGISTRY,
            managed_since,
        )
    };
    service.name = name.to_string();
    service.host_heuristic = host_heuristic.map(str::to_string);
    service
}

/// One host's tail of a managed unit's logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceLog {
    pub host: String,
    pub unit: String,
    /// The log file, or the journalctl invocation that produced the body.
    pub origin: String,
    pub body: String,
    /// The stderr half of the tail: the file it came from, or the reason
    /// there is none ("absent in plist", "<path> (empty)"). `None` where
    /// the channel merges the streams itself — journalctl already carries
    /// stderr, so Linux tails leave this unset.
    pub error_origin: Option<String>,
    pub error_body: String,
}

impl ServiceLog {
    pub fn to_json(&self) -> Value {
        let mut report = json!({
            "host": self.host,
            "unit": self.unit,
            "origin": self.origin,
            "lines": self.body.lines().collect::<Vec<&str>>(),
        });
        if let Some(error_origin) = &self.error_origin {
            report["error_origin"] = json!(error_origin);
            report["error_lines"] = json!(self.error_body.lines().collect::<Vec<&str>>());
        }
        report
    }
}

/// `service logs` on one host.
pub async fn tail_logs(
    target: &ComputeTarget,
    service: &ManagedService,
    lines: usize,
    runner: &Runner,
) -> Result<ServiceLog, DeployError> {
    tail_unit_logs(target, service.unit_id(), &service.path, lines, runner).await
}

/// The most log bytes one read may pull across the host channel.
///
/// A cap in lines cannot bound a transfer and cannot bound a diagnosis: on
/// 2026-09-05 `stado host unit-log … --lines 40000` returned 500 lines of the
/// object API's request log, which covered a few minutes, and an event at
/// 13:17 was already unreadable at 14:10. Bytes bound the transfer honestly.
pub const LOG_WINDOW_BYTES: usize = 4 * 1024 * 1024;

/// [`tail_logs`] addressed by the launchd label alone: for `host unit-log`,
/// whose caller names a unit the registry may never have declared, so the
/// plist search falls to the remote prelude's LaunchAgents/LaunchDaemons
/// order instead of a declared path.
pub async fn tail_unit_logs(
    target: &ComputeTarget,
    unit_id: &str,
    path: &str,
    lines: usize,
    runner: &Runner,
) -> Result<ServiceLog, DeployError> {
    // The stdout and stderr tails share the --lines budget, half each with
    // the odd line going to stdout; each side always gets at least one, so
    // `--lines 1` cannot blank stderr entirely.
    let out_lines = lines.saturating_sub(lines / 2).max(1);
    let err_lines = (lines / 2).max(1);
    let body = LOGS_BODY
        .replace("@LINES@", &shlex_quote(&lines.to_string()))
        .replace("@OUT_LINES@", &shlex_quote(&out_lines.to_string()))
        .replace("@ERR_LINES@", &shlex_quote(&err_lines.to_string()))
        // Bounded by bytes, not by trust in the line count: a unit that
        // writes a request per line fills any line budget in minutes, and a
        // reader who asks for more lines than the host hands back cannot tell
        // a quiet unit from a truncated window. 4 MiB is the ceiling on what
        // crosses the channel; the line count still selects within it.
        .replace("@MAX_BYTES@", &shlex_quote(&LOG_WINDOW_BYTES.to_string()));
    let script = remote_script(unit_id, "", path, &body)?;
    let report = run_remote(target, script, runner).await?;
    let Some((origin, tail)) = split_marker_body(&report.stdout, "STADO_LOG") else {
        return Err(DeployError(format!(
            "{}: {} log unavailable: {}",
            target.name,
            unit_id,
            report.failure()
        )));
    };
    let (body, error) = split_error_section(tail);
    let (error_origin, error_body) = match error {
        Some((error_origin, error_body)) => {
            (Some(error_origin.to_string()), error_body.to_string())
        }
        None => (None, String::new()),
    };
    Ok(ServiceLog {
        host: target.name.clone(),
        unit: unit_id.to_string(),
        origin: origin.to_string(),
        body: body.to_string(),
        error_origin,
        error_body,
    })
}

/// One host's unit file, fetched verbatim for local parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitFile {
    pub host: String,
    pub unit: String,
    pub path: String,
    pub kind: &'static str,
    pub content: String,
}

/// Read the actual executable from a launchd plist or systemd unit.
///
/// This is the typed counterpart to [`show_service`], whose detail is human
/// presentation and may append arguments and resolved-link annotations.
pub fn parse_unit_program(unit: &UnitFile) -> Result<Option<String>, DeployError> {
    if unit.kind == KIND_LAUNCHD {
        let document = parse_plist(&unit.content)?;
        // Program overrides argv[0] when launchd declares both.
        let program = document.get("Program").or_else(|| {
            document
                .get("ProgramArguments")
                .and_then(Value::as_array)
                .and_then(|arguments| arguments.first())
        });
        return program
            .map(|program| {
                program
                    .as_str()
                    .filter(|program| !program.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        DeployError(format!(
                            "{}: {} declares an empty or non-string program",
                            unit.host, unit.unit
                        ))
                    })
            })
            .transpose();
    }

    let mut in_service = false;
    let mut command = None;
    for line in logical_lines(&unit.content) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_service = name.trim() == "Service";
            continue;
        }
        if !in_service {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "ExecStart" {
            continue;
        }
        if value.trim().is_empty() {
            command = None;
            continue;
        }
        if command.is_some() {
            continue;
        }
        let Some(mut program) = split_words(value).into_iter().next() else {
            continue;
        };
        let prefix = program.len()
            - program
                .trim_start_matches(['@', '-', ':', '+', '!', '|'])
                .len();
        program.drain(..prefix);
        if !program.is_empty() {
            command = Some(program);
        }
    }
    Ok(command)
}

/// `service env`'s fetch: the unit and its overriding definitions on the host.
pub async fn fetch_unit_file(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<UnitFile, DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, UNIT_FILE_BODY)?;
    let report = run_remote(target, script, runner).await?;
    let Some((path, body)) = split_marker_body(&report.stdout, "STADO_UNITFILE") else {
        return Err(DeployError(format!(
            "{}: {} unit file unavailable: {}",
            target.name,
            service.unit_id(),
            report.failure()
        )));
    };
    Ok(UnitFile {
        host: target.name.clone(),
        unit: service.unit_id().to_string(),
        path: path.to_string(),
        kind: report.kind(),
        content: body.to_string(),
    })
}

/// How long one `file-sync` may take, for a payload of this size.
///
/// The content rides base64-inline inside the script body, so the transfer is
/// bounded by the channel's own clock rather than by any per-write timeout.
/// [`host_channel::run_script`] spends the fixed 120-second
/// [`host_channel::remote_timeout`], while `service file-sync --executable`
/// accepts payloads up to 96 MiB: every large file was therefore admitted by
/// the size check and then killed by the clock. Delivering a 35 MB Weles
/// worker release to a host's local release root failed exactly that way,
/// with `an upstream did not answer in time` after 138 seconds and nothing
/// written.
///
/// The floor stays the channel default, so small files behave exactly as
/// before; beyond that the budget grows with the bytes actually being sent —
/// one extra second per 256 KiB, which is roughly 4 seconds per megabyte and
/// comfortably slower than any link this fleet uses.
pub fn sync_timeout(content_len: usize) -> Duration {
    const BYTES_PER_SECOND_BUDGET: usize = 256 * 1024;
    host_channel::remote_timeout()
        + Duration::from_secs((content_len / BYTES_PER_SECOND_BUDGET) as u64)
}

/// Atomically replace one owner-only file on a managed host.
///
/// The content rides inside the approved channel's request body as base64,
/// never argv or output. The destination stays under the target account's
/// real home and a symlink is refused rather than followed.
pub async fn sync_service_file(
    target: &ComputeTarget,
    target_path: &str,
    content: &[u8],
    mode: u32,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if !matches!(mode, 0o600 | 0o700) {
        return Err(DeployError(format!(
            "service file mode must be 0600 or 0700, got {mode:04o}"
        )));
    }
    let body = r#"set -eu
fail() {
  printf 'STADO_SERVICE\tfile-sync\tfile_sync_failed\t%s\n' "$1"
  exit 0
}
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
target_path=$(printf '%s' '@TARGET_PATH_B64@' | /usr/bin/base64 "$decode") || exit 1
case "$target_path" in
  \$HOME/*) target_path="$HOME/${target_path#\$HOME/}" ;;
  "$HOME"/*) ;;
  *) fail 'target path must be under the target home' ;;
esac
if [ -e "$target_path" ] || [ -L "$target_path" ]; then
  [ -f "$target_path" ] && [ ! -L "$target_path" ] || fail 'target path is not a regular file'
fi
parent=$(/usr/bin/dirname "$target_path") || fail 'target parent unavailable'
/bin/mkdir -p "$parent" || fail 'cannot create target parent'
if ! /usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=os.path.realpath(sys.argv[2]); raise SystemExit(0 if os.path.commonpath((home,parent)) == home else 1)' "$HOME" "$parent"; then
  fail 'target parent escapes the target home'
fi
tmp="$target_path.stado-file-sync.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
umask u=rw,go=
printf '%s' '@CONTENT_B64@' | /usr/bin/base64 "$decode" > "$tmp" || fail 'cannot stage file'
/bin/chmod @MODE@ "$tmp" || fail 'cannot protect file'
/bin/mv -f "$tmp" "$target_path" || fail 'cannot install file'
trap - EXIT HUP INT TERM
printf 'STADO_SERVICE\tfile-sync\tfile_synced\t%s\n' "$target_path"
"#;
    let body = body
        .replace(
            "@TARGET_PATH_B64@",
            &STANDARD.encode(target_path.as_bytes()),
        )
        .replace("@CONTENT_B64@", &STANDARD.encode(content))
        .replace("@MODE@", &format!("{mode:04o}"));
    let output =
        host_channel::run_script_with_timeout(target, &body, sync_timeout(content.len()), runner)
            .await?;
    Ok(report_from(output))
}

/// Create, or atomically replace, one owner-only raw bearer file on a managed
/// host.
///
/// Binding a host's unit to the fleet object store needs
/// `WC_STADO_STORAGE_TOKEN_FILE` to name a file whose entire content is the
/// bearer: `queue/stado_object.rs::StadoObjectBackend::new` resolves a token
/// file and nothing else. Nothing here could create that file.
/// [`sync_service_secret`] writes an `env` assignment, and
/// [`remint_consumer_grant_on_host`] reconciles a grant against a token file
/// that is already on the host; between them the one remaining way to bind a
/// host to the fleet store was to hand-copy a secret onto it, which is exactly
/// what the fleet-wide "everything through Stado" rule exists to prevent. A
/// host that never got that file kept its `JobStorage` on a device-local store
/// instead, and a fleet claim written to a device store does not fail -- it
/// succeeds, and every other host reports the object absent.
///
/// The bearer rides inside the approved channel's request body as base64,
/// exactly the way [`sync_service_file`] carries file content: never an
/// argument vector, never stdout, never the clear text of the remote program.
/// The destination must resolve inside the target account's home and must not
/// be a symlink, its resolved parent must still be inside that home, and the
/// file is installed by renaming a mode-600 temporary file staged in the same
/// directory, so a reader never sees a half-written bearer. Unlike
/// [`set_env_key_on_host`] an absent destination is created -- that is the
/// whole point of this command -- but an absent parent directory is refused
/// rather than created: a bearer written into a directory this command invented
/// is a bearer nothing reads, and the typo that put it there would be reported
/// as a successful sync.
pub async fn write_token_file_on_host(
    target: &ComputeTarget,
    token_path: &str,
    secret: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_home_rooted_file(token_path, "token file")?;
    validate_secret_value(secret)?;
    let body = r#"set -eu
fail() {
  printf 'STADO_SERVICE\ttoken-file-sync\ttoken_file_sync_failed\t%s\n' "$1"
  exit 0
}
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
home=$HOME
token_path=$(printf '%s' '@TOKEN_PATH_B64@' | /usr/bin/base64 "$decode") || exit 1
case "$token_path" in
  '$HOME'/*) token_path="$home/${token_path#\$HOME/}" ;;
  "$home"/*) ;;
  *) fail 'token file must be inside the target home' ;;
esac
[ ! -L "$token_path" ] || fail 'token file cannot be a symlink'
if [ -e "$token_path" ]; then
  [ -f "$token_path" ] || fail 'token file is not a regular file'
fi
parent=$(/usr/bin/dirname "$token_path")
[ -d "$parent" ] || fail 'token file parent directory must already exist'
real_parent=$(/usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$parent")
/usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=sys.argv[2]; sys.exit(0 if os.path.commonpath((home,parent)) == home else 1)' "$home" "$real_parent" || fail 'resolved token file leaves the target home'
tmp="$parent/.stado-token-file-sync.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
umask u=rw,go=
printf '%s' '@TOKEN_B64@' | /usr/bin/base64 "$decode" > "$tmp" || fail 'cannot stage token file'
[ -s "$tmp" ] || fail 'staged token file is empty'
/bin/chmod 0600 "$tmp" || fail 'cannot protect token file'
/bin/mv -f "$tmp" "$token_path" || fail 'cannot install token file'
trap - EXIT HUP INT TERM
printf 'STADO_SERVICE\ttoken-file-sync\ttoken_file_synced\t%s\n' "$token_path"
"#;
    let body = body
        .replace("@TOKEN_PATH_B64@", &STANDARD.encode(token_path.as_bytes()))
        .replace("@TOKEN_B64@", &STANDARD.encode(secret.as_bytes()));
    let output = host_channel::run_script(target, &body, runner).await?;
    Ok(report_from(output))
}

/// Replace `consumer`'s complete grant against the target's authoritative
/// vault while preserving the bearer already held in `token_path`.
///
/// Both files stay on the managed host. Skarbiec reads the raw bearer itself
/// and records only its hash; the value never enters Stado output, argv, or
/// the control-plane process.
#[allow(clippy::too_many_arguments)]
pub async fn remint_consumer_grant_on_host(
    target: &ComputeTarget,
    consumer: &str,
    capabilities: &str,
    token_path: &str,
    vault_file: &str,
    ttl_seconds: u64,
    audience: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = r#"set -eu
fail() {
  printf 'STADO_SERVICE\t%s\tgrant_sync_failed\t%s\n' "$consumer" "$1"
  exit 0
}
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
vault=$(printf '%s' '@VAULT_B64@' | /usr/bin/base64 "$decode") || exit 1
consumer=$(printf '%s' '@CONSUMER_B64@' | /usr/bin/base64 "$decode") || exit 1
caps=$(printf '%s' '@CAPS_B64@' | /usr/bin/base64 "$decode") || exit 1
token_path=$(printf '%s' '@TOKEN_PATH_B64@' | /usr/bin/base64 "$decode") || exit 1
audience=$(printf '%s' '@AUDIENCE_B64@' | /usr/bin/base64 "$decode") || exit 1
case "$vault" in
  \$HOME/*) vault="$HOME/${vault#\$HOME/}" ;;
  "$HOME"/*) ;;
  *) fail 'vault path must be under the target home' ;;
esac
case "$token_path" in
  \$HOME/*) token_path="$HOME/${token_path#\$HOME/}" ;;
  "$HOME"/*) ;;
  *) fail 'token path must be under the target home' ;;
esac
[ -f "$vault" ] && [ ! -L "$vault" ] || fail 'authoritative vault is not a regular file'
[ -f "$token_path" ] && [ ! -L "$token_path" ] || fail 'token file is not a regular file'
/bin/chmod 600 "$token_path" || fail 'cannot protect token file'
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export GNUPGHOME="$HOME/.gnupg"
export SKARBIEC_VAULT_FILE="$vault"
if ! report=$("$HOME/.stado/bin/skarbiec" token-mint "$consumer" \
    --capabilities "$caps" \
    --token-file "$token_path" \
    --replace-capabilities \
    --ttl-seconds '@TTL_SECONDS@' \
    --audience "$audience" 2>&1); then
  fail "$report"
fi
printf 'STADO_SERVICE\t%s\tgrant_synced\t%s\n' "$consumer" "$token_path"
"#;
    let body = body
        .replace("@VAULT_B64@", &STANDARD.encode(vault_file.as_bytes()))
        .replace("@CONSUMER_B64@", &STANDARD.encode(consumer.as_bytes()))
        .replace("@CAPS_B64@", &STANDARD.encode(capabilities.as_bytes()))
        .replace("@TOKEN_PATH_B64@", &STANDARD.encode(token_path.as_bytes()))
        .replace("@AUDIENCE_B64@", &STANDARD.encode(audience.as_bytes()))
        .replace("@TTL_SECONDS@", &ttl_seconds.to_string());
    let output = host_channel::run_script(target, &body, runner).await?;
    Ok(report_from(output))
}

/// A systemd definition or one drop-in belonging to this declared unit.
pub fn is_systemd_env_file(service: &ManagedService, path: &str) -> bool {
    service.kind == KIND_SYSTEMD
        && !service.path.is_empty()
        && (path == service.path
            || path
                .strip_prefix(&service.path)
                .and_then(|suffix| suffix.strip_prefix(".d/"))
                .is_some_and(|name| {
                    name.ends_with(".conf")
                        && name.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                        })
                }))
}

/// Update a declared systemd environment assignment without cycling the unit.
/// `None` removes the assignment and an otherwise empty drop-in.
pub async fn set_unit_env_key_on_host(
    target: &ComputeTarget,
    service: &ManagedService,
    env_path: &str,
    key: &str,
    value: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if !is_systemd_env_file(service, env_path) {
        return Err(DeployError(
            "environment file does not belong to this systemd unit".into(),
        ));
    }
    let body = r###"stado_unit_env_writer() {
  if [ "$scope" = system ]; then
    stado_root "$@"
  elif [ "$service_uid" = "$uid" ]; then
    "$@"
  else
    "$sudo_bin" -n -u "$service_user" "$@"
  fi
}
if ! changed=$(stado_unit_env_writer /usr/bin/python3 - "$service_uid" 2>&1 <<'STADO_UNIT_ENV'
import base64, os, pathlib, re, shlex, stat, sys, tempfile

def decode(value):
    return base64.b64decode(value).decode("utf-8")

raw_path = decode("@ENV_PATH_B64@")
path = pathlib.Path(os.environ["HOME"]) / raw_path[6:] if raw_path.startswith("$HOME/") else pathlib.Path(raw_path)
for component in (path, *path.parents):
    if component.is_symlink():
        raise RuntimeError("unit environment path cannot contain a symlink")
before = path.stat()
if not stat.S_ISREG(before.st_mode) or before.st_uid != int(sys.argv[1]):
    raise RuntimeError("unit environment file must be regular and owned by the service account")
key = decode("@KEY_B64@")
value = decode("@VALUE_B64@") if @SET_VALUE@ else None
original = path.read_text()
entries, pending = [], []
for line in original.splitlines(keepends=True):
    pending.append(line)
    if line.rstrip("\r\n").endswith("\\"):
        continue
    entries.append("".join(pending))
    pending = []
if pending:
    entries.append("".join(pending))

words = re.compile(r"""(?:[^\s"'\\]|\\.|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')+""")
output, in_service, insertion = [], False, None
for raw in entries:
    logical = re.sub(r"\\\r?\n", " ", raw)
    stripped = logical.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        if in_service:
            insertion = len(output)
        in_service = stripped == "[Service]"
    assignment = re.match(r"^\s*Environment\s*=(.*)$", logical.rstrip("\r\n")) if in_service else None
    if assignment and assignment.group(1).strip():
        tokens = words.findall(assignment.group(1))
        retained = [token for token in tokens if shlex.split(token)[0].partition("=")[0] != key]
        if len(retained) != len(tokens):
            if retained:
                output.append("Environment=" + " ".join(retained) + "\n")
            continue
    output.append(raw)
if in_service:
    insertion = len(output)
if value is not None:
    if insertion is None:
        output.append("\n[Service]\n")
        insertion = len(output)
    if insertion and not output[insertion - 1].endswith("\n"):
        output[insertion - 1] += "\n"
    escaped = value.replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
    output.insert(insertion, 'Environment="' + key + "=" + escaped + '"\n')
updated = "".join(output)
empty_dropin = value is None and path.suffix == ".conf" and all(
    not line.strip() or line.strip() == "[Service]" for line in updated.splitlines()
)
if updated == original:
    print("unchanged")
elif empty_dropin:
    current = path.lstat()
    if (current.st_dev, current.st_ino, current.st_mtime_ns, current.st_size) != (before.st_dev, before.st_ino, before.st_mtime_ns, before.st_size):
        raise RuntimeError("unit environment file changed during the update")
    path.unlink()
    print("changed")
else:
    fd, temporary = tempfile.mkstemp(prefix=".stado-unit-env.", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            os.fchmod(stream.fileno(), stat.S_IMODE(before.st_mode))
            os.fchown(stream.fileno(), before.st_uid, before.st_gid)
            stream.write(updated)
            stream.flush()
            os.fsync(stream.fileno())
        current = path.lstat()
        if (current.st_dev, current.st_ino, current.st_mtime_ns, current.st_size) != (before.st_dev, before.st_ino, before.st_mtime_ns, before.st_size):
            raise RuntimeError("unit environment file changed during the update")
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    print("changed")
STADO_UNIT_ENV
); then
  say '@ACTION@_failed' "$(printf '%s' "$changed" | tr '\t\r\n' '   ')"
  exit 1
fi
if [ "$changed" = changed ] || [ "$(stado_systemctl show -p NeedDaemonReload --value "$unit")" = yes ]; then
  if ! stado_systemctl daemon-reload; then
    say '@ACTION@_failed' 'unit environment changed but systemd could not reload it'
    exit 1
  fi
fi
say '@ACTION@' "$changed; systemd definition refreshed without restarting the unit"
"###;
    let body = body
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@KEY_B64@", &STANDARD.encode(key.as_bytes()))
        .replace(
            "@VALUE_B64@",
            &STANDARD.encode(value.unwrap_or_default().as_bytes()),
        )
        .replace(
            "@SET_VALUE@",
            if value.is_some() { "True" } else { "False" },
        )
        .replace(
            "@ACTION@",
            if value.is_some() {
                "env_set"
            } else {
                "env_unset"
            },
        );
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// Atomically replace one assignment in an owner-controlled remote env file.
/// The value travels only inside the approved host-channel request body.
pub async fn set_env_key_on_host(
    target: &ComputeTarget,
    env_path: &str,
    key: &str,
    value: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = r#"set -eu
fail() { printf 'env_set_failed\t%s\n' "$1"; exit 1; }
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
home=$HOME
env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 "$decode")
case "$env_path" in
  '$HOME'/*) env_path="$home/${env_path#\$HOME/}" ;;
  "$home"/*) ;;
  /*) fail 'target must be inside the target home' ;;
  *) env_path="$home/$env_path" ;;
esac
case "$env_path" in "$home"/*) ;; *) fail 'target must be inside the target home' ;; esac
[ ! -L "$env_path" ] || fail 'target cannot be a symlink'
[ -f "$env_path" ] || fail 'environment file must already exist'
parent=$(/usr/bin/dirname "$env_path")
real_parent=$(/usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$parent")
/usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=sys.argv[2]; sys.exit(0 if os.path.commonpath((home,parent)) == home else 1)' "$home" "$real_parent" || fail 'resolved target leaves the target home'
key=$(printf '%s' '@KEY_B64@' | /usr/bin/base64 "$decode")
value=$(printf '%s' '@VALUE_B64@' | /usr/bin/base64 "$decode")
tmp="$parent/.stado-env-set.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
/usr/bin/awk -v key="$key" '$0 !~ "^" key "=" { print }' "$env_path" > "$tmp"
printf '%s=%s\n' "$key" "$value" >> "$tmp"
/bin/chmod 0600 "$tmp"
/bin/mv -f "$tmp" "$env_path"
trap - EXIT HUP INT TERM
printf 'STADO_SERVICE\tenv-set\tenv_set\t%s\n' "$env_path"
"#;
    let body = body
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@KEY_B64@", &STANDARD.encode(key.as_bytes()))
        .replace("@VALUE_B64@", &STANDARD.encode(value.as_bytes()));
    let output = host_channel::run_script(target, &body, runner).await?;
    Ok(report_from(output))
}

/// Atomically remove one assignment from an owner-controlled remote env file.
pub async fn unset_env_key_on_host(
    target: &ComputeTarget,
    env_path: &str,
    key: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = r#"set -eu
fail() { printf 'env_unset_failed\t%s\n' "$1"; exit 1; }
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
home=$HOME
env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 "$decode")
case "$env_path" in
  '$HOME'/*) env_path="$home/${env_path#\$HOME/}" ;;
  "$home"/*) ;;
  /*) fail 'target must be inside the target home' ;;
  *) env_path="$home/$env_path" ;;
esac
case "$env_path" in "$home"/*) ;; *) fail 'target must be inside the target home' ;; esac
[ ! -L "$env_path" ] || fail 'target cannot be a symlink'
[ -f "$env_path" ] || fail 'environment file must already exist'
parent=$(/usr/bin/dirname "$env_path")
real_parent=$(/usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$parent")
/usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=sys.argv[2]; sys.exit(0 if os.path.commonpath((home,parent)) == home else 1)' "$home" "$real_parent" || fail 'resolved target leaves the target home'
key=$(printf '%s' '@KEY_B64@' | /usr/bin/base64 "$decode")
tmp="$parent/.stado-env-unset.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
/usr/bin/awk -v key="$key" '$0 !~ "^" key "=" { print }' "$env_path" > "$tmp"
/bin/chmod 0600 "$tmp"
/bin/mv -f "$tmp" "$env_path"
trap - EXIT HUP INT TERM
printf 'STADO_SERVICE\tenv-unset\tenv_unset\t%s\n' "$env_path"
"#;
    let body = body
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@KEY_B64@", &STANDARD.encode(key.as_bytes()));
    let output = host_channel::run_script(target, &body, runner).await?;
    Ok(report_from(output))
}

/// Write one vault item field on the host using its own Skarbiec binary
/// and vault file. The value file must already exist on the host.
pub async fn set_item_field_on_host(
    target: &ComputeTarget,
    item: &str,
    field: &str,
    value_file: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = r#"set -eu
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export GNUPGHOME="$HOME/.gnupg"
"$HOME/.stado/bin/skarbiec" set-json "$STADO_ITEM" --field "$STADO_FIELD" --from-file "$STADO_FROM"
echo 'STADO_ITEM_SET	ok'"#;
    let body = body
        .replace("{vault_file}", "")
        .replace("$STADO_ITEM", &shlex_quote(item))
        .replace("$STADO_FIELD", &shlex_quote(field))
        .replace("$STADO_FROM", &shlex_quote(value_file));
    let output = host_channel::run_script(target, &body, runner).await?;
    if !output.stdout.contains("STADO_ITEM_SET") {
        return Err(DeployError(format!(
            "{}: could not set {}.{}: {}",
            target.name,
            item,
            field,
            output.stderr.trim_end()
        )));
    }
    Ok(report_from(output))
}

// ---------------------------------------------------------------------------
// Registry document mutation (pure; the write goes through cli/registry.rs)
// ---------------------------------------------------------------------------

/// Borrow one kind=local target object out of the raw canonical document.
fn target_entry<'a>(
    document: &'a mut Value,
    host: &str,
) -> Result<&'a mut Map<String, Value>, DeployError> {
    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| DeployError("registry.targets: must be an array".to_string()))?;
    let entry = targets
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(host))
        .ok_or_else(|| {
            DeployError(format!(
                "target {} is not in the canonical registry",
                py_str_repr(host)
            ))
        })?;
    let entry = entry
        .as_object_mut()
        .ok_or_else(|| DeployError("registry target must be an object".to_string()))?;
    if entry.get("kind").and_then(Value::as_str) != Some("local") {
        return Err(DeployError(format!(
            "target {} is not a local host",
            py_str_repr(host)
        )));
    }
    Ok(entry)
}

/// Declare a service in the canonical document.
///
/// Pure by design: the caller reads the document with its generation through
/// `cli/registry.rs::{commit_document, fetch_versioned_document}`, applies
/// this, and writes it back conditionally on that generation, which validates
/// the whole document before it writes anything.
pub fn add_service(document: &mut Value, service: &ManagedService) -> Result<(), DeployError> {
    let entry = target_entry(document, &service.host)?;
    let declared = entry
        .entry(SERVICES_KEY)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| {
            DeployError(format!(
                "registry target {} has a non-array {SERVICES_KEY} key",
                py_str_repr(&service.host)
            ))
        })?;
    let taken = declared
        .iter()
        .filter_map(Value::as_object)
        .map(|record| ManagedService::from_record(&service.host, record))
        .any(|existing| existing.matches(service.unit_id()) || existing.matches(&service.name));
    if taken {
        return Err(DeployError(format!(
            "the registry already manages {} on {}",
            py_str_repr(service.unit_id()),
            py_str_repr(&service.host)
        )));
    }
    declared.push(service.to_record());
    Ok(())
}

/// Replace one registry-managed service after a host observation corrected its
/// unit identity or file path. The match is by logical name or stable unit id;
/// recovery-sourced services are never written into the registry.
pub fn replace_service(document: &mut Value, service: &ManagedService) -> Result<(), DeployError> {
    let entry = target_entry(document, &service.host)?;
    let declared = entry
        .get_mut(SERVICES_KEY)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            DeployError(format!(
                "{} declares no managed services",
                py_str_repr(&service.host)
            ))
        })?;
    let record = declared
        .iter_mut()
        .find(|record| {
            record.as_object().is_some_and(|record| {
                let existing = ManagedService::from_record(&service.host, record);
                existing.matches(&service.name) || existing.matches(service.unit_id())
            })
        })
        .ok_or_else(|| {
            DeployError(format!(
                "{} is not a registry-managed service on {}",
                py_str_repr(&service.name),
                py_str_repr(&service.host)
            ))
        })?;
    *record = service.to_record();
    Ok(())
}

/// Attach product onboarding metadata to one already managed service.
pub fn set_service_onboarding(
    document: &mut Value,
    host: &str,
    service: &str,
    onboarding: OnboardingProduct,
) -> Result<ManagedService, DeployError> {
    let entry = target_entry(document, host)?;
    let declared = entry
        .get_mut(SERVICES_KEY)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            DeployError(format!(
                "{} declares no managed services",
                py_str_repr(host)
            ))
        })?;
    let record = declared
        .iter_mut()
        .find(|record| {
            record
                .as_object()
                .is_some_and(|record| ManagedService::from_record(host, record).matches(service))
        })
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            DeployError(format!(
                "{} is not a registry-managed service on {}",
                py_str_repr(service),
                py_str_repr(host)
            ))
        })?;
    record.insert(
        "onboarding".to_string(),
        serde_json::to_value(&onboarding)
            .map_err(|error| DeployError(format!("invalid onboarding product: {error}")))?,
    );
    Ok(ManagedService::from_record(host, record))
}

/// Undeclare a service. Removing the last one drops the key entirely, so a
/// host with nothing declared reads the same as one that never declared
/// anything.
pub fn remove_service(
    document: &mut Value,
    host: &str,
    unit: &str,
) -> Result<ManagedService, DeployError> {
    let entry = target_entry(document, host)?;
    let declared = entry
        .get_mut(SERVICES_KEY)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            DeployError(format!(
                "{} declares no managed services",
                py_str_repr(host)
            ))
        })?;
    // Position over the array itself, not over a filtered view: a record
    // that is not an object still occupies a slot, and an index taken from
    // a filtered sequence would delete the wrong one.
    let found = declared.iter().position(|record| {
        record
            .as_object()
            .is_some_and(|record| ManagedService::from_record(host, record).matches(unit))
    });
    let Some(index) = found else {
        return Err(DeployError(format!(
            "{} is not a registry-managed service on {}",
            py_str_repr(unit),
            py_str_repr(host)
        )));
    };
    let removed = declared.remove(index);
    let now_empty = declared.is_empty();
    let removed = removed
        .as_object()
        .map(|record| ManagedService::from_record(host, record))
        .unwrap_or_default();
    if now_empty {
        entry.remove(SERVICES_KEY);
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Unit-file parsing and secret redaction
// ---------------------------------------------------------------------------

/// Base of a hexadecimal character reference, spelled as a type constant
/// because this crate's edit policy forbids bare numeric literals — the
/// same technique `cli/mod.rs::default_mail_results` uses to derive its
/// default from `u8::BITS`.
const HEX_RADIX: u32 = u16::BITS;

/// Case-insensitive "this variable holds a credential" test.
///
/// Built the way `artifacts/validation.rs::sensitive_query_key` is — one
/// cached regex with `(^|[-_])…($|[-_])` boundaries — so a lookalike such
/// as `TOKENIZERS_PARALLELISM` or `WELES_KEYWORD_ROOT` is not swept up,
/// while `HF_TOKEN` and `AWS_SECRET_ACCESS_KEY` are.
///
/// It deliberately over-matches in one direction: a name like
/// `GOOGLE_APPLICATION_CREDENTIALS` holds a path, not a secret, and is
/// redacted anyway. The alternative is an allowlist of credential-shaped
/// names that happen to be safe, and the first entry someone adds to it
/// wrong prints a live token.
static SECRET_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(^|[-_])(api[-_]?key|auth|authorization|bearer|credential|credentials|key|keys|passwd|password|private[-_]?key|pwd|secret|secrets|session|signature|token|tokens)($|[-_])",
    )
    .expect("static regex compiles")
});

/// The value as it may be printed. Credential-shaped names collapse to
/// [`REDACTED`]; an empty value stays empty, because "unset" is not a
/// secret and hiding it would misreport the unit's environment.
pub fn redact_secret_value(name: &str, value: &str) -> String {
    if value.is_empty() || !SECRET_NAME.is_match(name) {
        return value.to_string();
    }
    REDACTED.to_string()
}

/// One managed unit's effective environment, already redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceEnv {
    pub host: String,
    pub unit: String,
    pub path: String,
    pub kind: String,
    /// Variable name to printable value, in unit-file order.
    pub env: Vec<(String, String)>,
    /// systemd `EnvironmentFile=` references. Their contents are NOT read:
    /// they are a pointer to more environment, and reporting them is how
    /// the operator learns this picture is partial.
    pub environment_files: Vec<String>,
}

impl ServiceEnv {
    pub fn to_json(&self) -> Value {
        let env: Map<String, Value> = self
            .env
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect();
        json!({
            "host": self.host,
            "unit": self.unit,
            "path": self.path,
            "kind": self.kind,
            "environment": env,
            "environment_files": self.environment_files,
        })
    }
}

/// Parse a fetched unit file into its redacted effective environment.
pub fn unit_environment(unit: &UnitFile) -> Result<ServiceEnv, DeployError> {
    let (env, environment_files) = if unit.kind == KIND_LAUNCHD {
        (plist_env(&parse_plist(&unit.content)?), Vec::new())
    } else {
        let parsed = parse_systemd_unit(&unit.content);
        (parsed.env, parsed.environment_files)
    };
    let env = env
        .into_iter()
        .map(|(key, value)| {
            let value = redact_secret_value(&key, &value);
            (key, value)
        })
        .collect();
    Ok(ServiceEnv {
        host: unit.host.clone(),
        unit: unit.unit.clone(),
        path: unit.path.clone(),
        kind: unit.kind.to_string(),
        env,
        environment_files,
    })
}

/// `EnvironmentVariables` out of a parsed property list, in file order.
pub fn plist_env(document: &Value) -> Vec<(String, String)> {
    document
        .get("EnvironmentVariables")
        .and_then(Value::as_object)
        .map(|env| {
            env.iter()
                .map(|(key, value)| (key.clone(), scalar_text(value)))
                .collect()
        })
        .unwrap_or_default()
}

/// A plist scalar as an operator sees it: strings raw, everything else in
/// its JSON spelling.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// One XML token: an element boundary or a run of character data.
enum Token<'a> {
    Open(&'a str),
    Close(&'a str),
    Empty(&'a str),
    Text(String),
}

/// Split an XML property list into element boundaries and text runs.
/// Declarations, the DOCTYPE and comments are skipped; attributes on an
/// open tag are dropped.
fn tokenize(text: &str) -> Result<Vec<Token<'_>>, DeployError> {
    let mut tokens: Vec<Token<'_>> = Vec::new();
    let mut rest = text;
    loop {
        let Some((before, after)) = rest.split_once('<') else {
            push_text(&mut tokens, rest);
            return Ok(tokens);
        };
        push_text(&mut tokens, before);
        if let Some(comment) = after.strip_prefix("!--") {
            let Some((_, tail)) = comment.split_once("-->") else {
                return Err(malformed("unterminated comment"));
            };
            rest = tail;
            continue;
        }
        let Some((tag, tail)) = after.split_once('>') else {
            return Err(malformed("unterminated tag"));
        };
        rest = tail;
        let tag = tag.trim();
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            tokens.push(Token::Close(name.trim()));
        } else if let Some(name) = tag.strip_suffix('/') {
            tokens.push(Token::Empty(name.trim()));
        } else {
            tokens.push(Token::Open(
                tag.split_whitespace().next().unwrap_or_default(),
            ));
        }
    }
}

fn push_text<'a>(tokens: &mut Vec<Token<'a>>, text: &str) {
    if !text.trim().is_empty() {
        tokens.push(Token::Text(decode_entities(text)));
    }
}

fn malformed(reason: &str) -> DeployError {
    DeployError(format!(
        "unit file is not a well-formed XML property list: {reason}"
    ))
}

/// Named and numeric XML character references. An unrecognised `&...;` run
/// is left verbatim rather than dropped: a plist value is operator data,
/// and mangling it silently is worse than showing it raw.
fn decode_entities(text: &str) -> String {
    let Some((head, mut rest)) = text.split_once('&') else {
        return text.to_string();
    };
    let mut out = head.to_string();
    loop {
        let Some((entity, tail)) = rest.split_once(';') else {
            out.push('&');
            out.push_str(rest);
            return out;
        };
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => match numeric_entity(entity) {
                Some(ch) => out.push(ch),
                None => {
                    out.push('&');
                    out.push_str(entity);
                    out.push(';');
                }
            },
        }
        match tail.split_once('&') {
            Some((plain, next)) => {
                out.push_str(plain);
                rest = next;
            }
            None => {
                out.push_str(tail);
                return out;
            }
        }
    }
}

/// A decimal (`#NN`) or hexadecimal (`#xHH`) character reference.
fn numeric_entity(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let code = match digits
        .strip_prefix('x')
        .or_else(|| digits.strip_prefix('X'))
    {
        Some(hex) => u32::from_str_radix(hex, HEX_RADIX).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

/// A container being filled while walking the token stream.
enum Frame {
    Dict(Map<String, Value>, Option<String>),
    Array(Vec<Value>),
}

/// Read an Apple XML property list into a [`Value`].
///
/// Covers the element set a launchd unit uses — `dict`, `array`, `string`,
/// `integer`, `real`, `true`, `false`, `data`, `date`. A binary plist is
/// not XML and is reported as unreadable rather than parsed as an empty
/// document, so `service env` can never claim a unit has no environment
/// when it simply could not read the file.
pub fn parse_plist(text: &str) -> Result<Value, DeployError> {
    if text.trim_start().starts_with("bplist") {
        return Err(DeployError(
            "unit file is a binary property list; convert it with `plutil -convert xml1`"
                .to_string(),
        ));
    }
    let tokens = tokenize(text)?;
    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Value> = None;
    let mut buffer = String::new();
    let mut reading_scalar = false;

    for token in &tokens {
        match token {
            Token::Text(text) => {
                if reading_scalar {
                    buffer.push_str(text);
                }
            }
            Token::Open(name) => match *name {
                "dict" => stack.push(Frame::Dict(Map::new(), None)),
                "array" => stack.push(Frame::Array(Vec::new())),
                "key" | "string" | "integer" | "real" | "date" | "data" => {
                    reading_scalar = true;
                    buffer.clear();
                }
                _ => {}
            },
            Token::Empty(name) => match *name {
                "true" => place(&mut stack, &mut root, Value::Bool(true))?,
                "false" => place(&mut stack, &mut root, Value::Bool(false))?,
                "dict" => place(&mut stack, &mut root, Value::Object(Map::new()))?,
                "array" => place(&mut stack, &mut root, Value::Array(Vec::new()))?,
                "string" => place(&mut stack, &mut root, Value::String(String::new()))?,
                _ => {}
            },
            Token::Close(name) => {
                match *name {
                    "dict" | "array" => {
                        let frame = stack
                            .pop()
                            .ok_or_else(|| malformed("unbalanced container"))?;
                        let value = match frame {
                            Frame::Dict(map, _) => Value::Object(map),
                            Frame::Array(items) => Value::Array(items),
                        };
                        place(&mut stack, &mut root, value)?;
                    }
                    "key" => {
                        let Some(Frame::Dict(_, pending)) = stack.last_mut() else {
                            return Err(malformed("<key> outside a <dict>"));
                        };
                        *pending = Some(std::mem::take(&mut buffer));
                    }
                    "string" | "date" | "data" => {
                        let value = Value::String(std::mem::take(&mut buffer));
                        place(&mut stack, &mut root, value)?;
                    }
                    "integer" => {
                        let parsed = buffer
                            .trim()
                            .parse::<i64>()
                            .map_err(|_| malformed("<integer> is not an integer"))?;
                        buffer.clear();
                        place(&mut stack, &mut root, json!(parsed))?;
                    }
                    "real" => {
                        let parsed = buffer
                            .trim()
                            .parse::<f64>()
                            .map_err(|_| malformed("<real> is not a number"))?;
                        buffer.clear();
                        place(&mut stack, &mut root, json!(parsed))?;
                    }
                    _ => {}
                }
                reading_scalar = false;
            }
        }
    }
    root.ok_or_else(|| malformed("no root element"))
}

/// Attach a finished value to the container being filled, or make it the
/// document root when there is none.
fn place(stack: &mut [Frame], root: &mut Option<Value>, value: Value) -> Result<(), DeployError> {
    match stack.last_mut() {
        Some(Frame::Dict(map, pending)) => {
            let key = pending
                .take()
                .ok_or_else(|| malformed("<dict> value without a <key>"))?;
            map.insert(key, value);
        }
        Some(Frame::Array(items)) => items.push(value),
        None => *root = Some(value),
    }
    Ok(())
}

/// The environment a `systemd --user` unit declares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemdUnit {
    pub env: Vec<(String, String)>,
    pub environment_files: Vec<String>,
}

/// Read `[Service]`'s `Environment=` and `EnvironmentFile=` directives.
///
/// Follows systemd's own rules for the cases that change the answer:
/// backslash line continuations are joined, a bare `Environment=` clears
/// everything set before it, and one directive may carry several
/// quoted assignments.
pub fn parse_systemd_unit(text: &str) -> SystemdUnit {
    let mut parsed = SystemdUnit::default();
    let mut section = String::new();
    for line in logical_lines(text) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_string();
            continue;
        }
        if section != "Service" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Environment" if value.trim().is_empty() => parsed.env.clear(),
            "Environment" => {
                for word in split_words(value) {
                    if let Some((name, setting)) = word.split_once('=') {
                        parsed.env.push((name.to_string(), setting.to_string()));
                    }
                }
            }
            "EnvironmentFile" if value.trim().is_empty() => parsed.environment_files.clear(),
            "EnvironmentFile" => parsed.environment_files.push(value.trim().to_string()),
            _ => {}
        }
    }
    parsed
}

/// Join backslash line continuations into logical directives.
fn logical_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut pending = String::new();
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        if let Some(head) = trimmed.strip_suffix('\\') {
            pending.push_str(head.trim_end());
            pending.push(' ');
            continue;
        }
        pending.push_str(trimmed);
        lines.push(std::mem::take(&mut pending));
    }
    if !pending.is_empty() {
        lines.push(pending);
    }
    lines
}

/// Split a directive value into whitespace-separated words, honouring both
/// quote characters, so a quoted assignment whose value contains a space
/// stays one assignment instead of splitting into two words.
fn split_words(value: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for ch in value.chars() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => {
                current.push(ch);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

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
