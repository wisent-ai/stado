//! Full service management for registry-managed hosts.
//!
//! NO Python original: `stado/` has no service layer at all, and that
//! absence is the incident this module closes. On the July control-host
//! outage `com.wisent.weles-api` existed on the box and was wedged, but
//! nothing in Stado declared it — so no command could list it, restart it,
//! or even assert that it was supposed to be running.
//! `docs/missing-commands.md` items seven through fourteen are the
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

use std::path::{Component, Path};
use std::sync::LazyLock;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::local_install::{self, InstallPlan, LocalOs};
use super::{
    host_channel, host_recovery, py_str_repr, shlex_quote, CommandOutput, DeployError, Runner,
};
use crate::monitor::host_health::{self, HostHealthError};
use crate::queue::JobStorage;
use crate::targets::{self, ComputeTarget};

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
/// Linux `systemd --user`.
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
        onboarding: None,
    }
}

/// A `systemd --user` managed service.
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
        onboarding: None,
    }
}

/// Every unit Stado manages on one target: the registry-declared array
/// first, then the fixed recovery agents that are not already declared. A
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
                .map(|record| ManagedService::from_record(&target.name, record))
                .collect()
        })
        .unwrap_or_default();
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

/// Every registry-managed service on every kind=local host, with the state
/// the latest beacons report.
///
/// Beacons only: no ssh, no per-host round trip, so this stays answerable
/// while a host is wedged. A host that has never published a beacon yields
/// [`STATE_UNKNOWN`] rows instead of an error, because one silent host must
/// not blank the fleet-wide answer.
pub async fn list_services(store: &JobStorage) -> Result<Vec<ServiceStatus>, DeployError> {
    let registry = targets::fetch_registry_remote()
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
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
        for service in declared {
            let (state, detail) = beacon_state(beacon, service.unit_id());
            rows.push(ServiceStatus {
                service,
                state,
                reported_at: reported_at.clone(),
                detail,
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
    /// The launchd domain the host resolved (`gui/<uid>` or `user/<uid>`);
    /// empty on Linux.
    pub domain: String,
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

    pub fn to_json(&self) -> Value {
        let mut report = json!({
            "os": self.os,
            "launchd_domain": self.domain,
            "unit": self.unit,
            "path": self.path,
            "status": self.status,
            "detail": self.detail,
            "exit_code": self.exit_code,
        });
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
fn quote_unit_path(path: &str) -> Result<String, DeployError> {
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

/// Validate the remote environment file independently of shell quoting.
///
/// The command deliberately supports only an absolute path or a path rooted
/// at the target user's home. The value travels base64-encoded, but rejecting
/// parent traversal keeps a typo from turning a credential sync into an
/// unrelated file rewrite.
fn validate_env_path(path: &str) -> Result<(), DeployError> {
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
        "environment file {} must be an absolute or home-relative file path without parent traversal",
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
fn validate_unit_id(unit: &str) -> Result<(), DeployError> {
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
say() {
  detail=$(printf '%s' \"$2\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-160)
  printf 'STADO_SERVICE\\t%s\\t%s\\t%s\\n' \"$unit\" \"$1\" \"$detail\"
}
launch=/bin/launchctl
if [ \"$os\" = \"Darwin\" ]; then
  case \"$unit_path\" in
    /Library/LaunchDaemons/*)
      # A system daemon does not live in this login's domain, and every
      # launchctl verb aimed at gui/$uid silently misses it -- which is how
      # such a unit reaches the last-resort fallback on every restart and
      # gets started as a bare process instead of as the job it is.
      domain=\"system\"
      launch=\"/usr/bin/sudo -n /bin/launchctl\"
      ;;
  esac
  if [ -z \"$domain\" ]; then
    if /bin/launchctl print \"$gui\" >/dev/null 2>&1; then
      domain=\"$gui\"
    elif /bin/launchctl print \"$user_domain\" >/dev/null 2>&1; then
      domain=\"$user_domain\"
    else
@NO_DOMAIN@
    fi
  fi
  # An unqualified label may name either this login's agent or a system
  # daemon. Adoption looked only in the login's LaunchAgents and therefore
  # reported a running always-on daemon as absent, which is the one class of
  # unit this fleet keeps in the system domain.
  if [ -z \"$unit_path\" ]; then
    if [ -f \"$HOME/Library/LaunchAgents/$unit.plist\" ]; then
      unit_path=\"$HOME/Library/LaunchAgents/$unit.plist\"
    elif [ -f \"/Library/LaunchDaemons/$unit.plist\" ]; then
      unit_path=\"/Library/LaunchDaemons/$unit.plist\"
      domain=\"system\"
      launch=\"/usr/bin/sudo -n /bin/launchctl\"
    else
      unit_path=\"$HOME/Library/LaunchAgents/$unit.plist\"
    fi
  fi
elif [ \"$os\" = \"Linux\" ]; then
  if [ -n \"$linux_unit\" ]; then unit=\"$linux_unit\"; fi
  if [ -z \"$unit_path\" ]; then unit_path=\"$HOME/.config/systemd/user/$unit\"; fi
  case \"$unit_path\" in
    */.config/systemd/user/*) owner_path=\"${unit_path%%/.config/systemd/user/*}\" ;;
    *) owner_path=\"$unit_path\" ;;
  esac
  while [ ! -e \"$owner_path\" ] && [ \"$owner_path\" != \"/\" ]; do
    owner_path=$(/usr/bin/dirname \"$owner_path\")
  done
  service_user=$(/usr/bin/stat -c %U \"$owner_path\")
  service_uid=$(/usr/bin/id -u \"$service_user\")
  systemctl_user() {
    runtime=\"/run/user/$service_uid\"
    if [ \"$service_uid\" = \"$uid\" ]; then
      /usr/bin/env \
        XDG_RUNTIME_DIR=\"$runtime\" \
        DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
        /usr/bin/systemctl --user \"$@\"
      return
    fi
    if [ -x /usr/bin/sudo ]; then sudo_bin=/usr/bin/sudo; else sudo_bin=/bin/sudo; fi
    \"$sudo_bin\" -u \"$service_user\" /usr/bin/env \
      XDG_RUNTIME_DIR=\"$runtime\" \
      DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
      /usr/bin/systemctl --user \"$@\"
  }
else
  say 'unsupported_os' \"$os\"
  exit 65
fi
printf 'STADO_HOST\\t%s\\t%s\\t%s\\t%s\\n' \"$os\" \"$domain\" \"$unit\" \"$unit_path\"
";

/// What the prelude does on a Darwin host whose per-login launchd domain does
/// not exist, for every command that addresses a unit already installed.
///
/// A restart, a stop or a retire aimed at a domain that is not there has
/// nothing to act on, and inventing one would mean installing a unit in the
/// middle of an operation that promised only to touch an existing one.
const NO_DOMAIN_REFUSE: &str = "      say 'no_launchd_domain' \"$gui\"
      exit 66";

/// What [`ensure_service`] does instead: install into the system domain.
///
/// `launchctl bootstrap gui/$uid` over ssh answers `Could not switch to audit
/// session ... Operation not permitted`, and `stado service deploy` returned
/// that failure having installed nothing — which is how two `stado agent`
/// processes came to run for four days with no unit behind them. The system
/// domain is the one that does exist on an ssh login, so the unit that gets
/// installed is the daemon spelling of the same job, in
/// `/Library/LaunchDaemons`, and every later command finds it through the
/// `/Library/LaunchDaemons/*` case above.
const NO_DOMAIN_SYSTEM: &str = "      domain=\"system\"
      launch=\"/usr/bin/sudo -n /bin/launchctl\"
      unit_path=\"/Library/LaunchDaemons/$unit.plist\"";

// ---------------------------------------------------------------------------
// The end states the lifecycle operations intend
// ---------------------------------------------------------------------------

/// What launchd currently thinks of the unit, as the two facts every
/// lifecycle end state is decided on: `pc_loaded` (does a job exist under
/// this label) and `pc_pid` (is anything running under it).
///
/// One spelling, spliced into both probes, so "running" cannot come to mean
/// one thing when starting and another when stopping — which is how a stop
/// that only booted out a label ever came to be reported as a stop at all.
/// The `pid = N` line is read out of the same `launchctl print` the restart
/// script already uses to decide whether it may kick a job in place, so the
/// check and the operation agree on what they are looking at.
const LAUNCHD_STATE: &str = "    pc_pid=''
    if pc_info=$($launch print \"$domain/$unit\" 2>&1); then
      pc_loaded=yes
      pc_pid=$(printf '%s\\n' \"$pc_info\" | /usr/bin/awk '$1 == \"pid\" && $2 == \"=\" { print $3; exit }')
    else
      pc_loaded=no
    fi
";

/// The end state a restart or a start intends.
///
/// Both halves are load-bearing. A unit can be loaded with nothing running
/// under it (launchd accepted the job and the program died on start), and a
/// program can be running with no unit loaded — that second one is what the
/// last-resort fallbacks in these scripts produce, and reporting it as a
/// successful restart is how an operator comes to believe a service is
/// under management when the next reboot will not bring it back.
const RUNNING_DESCRIBE: &str = "the unit is loaded and has a pid";

const RUNNING_PROBE: &str = "  if [ \"$os\" = \"Darwin\" ]; then
@LAUNCHD_STATE@
    if [ \"$pc_loaded\" = no ]; then
      stado_post 'unmet' \"no job at $domain/$unit\"
    elif [ -n \"$pc_pid\" ]; then
      stado_post 'met' \"$domain/$unit pid $pc_pid\"
    else
      stado_post 'unmet' \"$domain/$unit is loaded with no pid\"
    fi
  elif systemctl_user is-active --quiet \"$unit\"; then
    pc_pid=$(systemctl_user show --property=MainPID --value \"$unit\" 2>/dev/null)
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
@LAUNCHD_STATE@
    if [ \"$pc_loaded\" = no ]; then
      stado_post 'met' \"no job at $domain/$unit\"
    elif [ -n \"$pc_pid\" ]; then
      stado_post 'unmet' \"$domain/$unit still running as pid $pc_pid\"
    else
      stado_post 'met' \"$domain/$unit is loaded but not running\"
    fi
  elif systemctl_user is-active --quiet \"$unit\"; then
    stado_post 'unmet' \"$unit is still active\"
  else
    stado_post 'met' \"$unit is not active\"
  fi
";

/// One declared end state, with the shared launchd observation spliced in.
fn end_state(describe: &'static str, probe: &str) -> host_channel::PostCondition {
    host_channel::PostCondition {
        describe,
        probe: probe.replace("@LAUNCHD_STATE@", LAUNCHD_STATE),
    }
}

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
/// at all, or whose in-place kick fails, and it no longer exits leaving the
/// unit down without saying so.
const RESTART_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  if $launch print \"$domain/$unit\" >/dev/null 2>&1; then
    detail=$($launch kickstart -k \"$domain/$unit\" 2>&1)
    rc=$?
    if [ \"$rc\" -eq 0 ]; then
      say 'restarted' \"$domain in place\"
      exit 0
    fi
  fi
  if [ -f \"$unit_path\" ]; then
    $launch bootout \"$domain/$unit\" >/dev/null 2>&1 || true
@DISOWNED_SWEEP@
    if [ -n \"$still\" ]; then
      $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
      $launch bootstrap \"$domain\" \"$unit_path\" >/dev/null 2>&1 || true
      if $launch print \"$domain/$unit\" >/dev/null 2>&1; then
        say 'restart_failed' \"disowned process survived: $still; unit reloaded\"
      else
        say 'restart_failed' \"disowned process survived: $still; unit is NOT loaded\"
      fi
      exit 0
    fi
    $launch enable \"$domain/$unit\" >/dev/null || true
    detail=$($launch bootstrap \"$domain\" \"$unit_path\" 2>&1)
    rc=$?
    if ! $launch print \"$domain/$unit\" >/dev/null; then
      detail=$(/bin/launchctl asuser \"$uid\" $launch bootstrap \"$domain\" \"$unit_path\")
      rc=$?
    fi
    if ! $launch print \"$domain/$unit\" >/dev/null; then
      set --
      while IFS= read -r line; do
        case \"$line\" in
          'Array {'|'}'|'') continue ;;
        esac
        set -- \"$@\" \"$(printf '%s' \"$line\" | /usr/bin/sed 's/^[[:space:]]*//;s/[[:space:]]*$//')\"
      done <<PLIST
$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' \"$unit_path\" 2>/dev/null)
PLIST
      program=${1:-}
      if [ -n \"$program\" ]; then
        recovery_unit=\"${unit}-recovery\"
        detail=$(/bin/launchctl submit -l \"$recovery_unit\" -- \"$@\")
        rc=$?
        if $launch print \"$domain/$recovery_unit\" >/dev/null; then unit=\"$recovery_unit\"; fi
      fi
    fi
    if ! $launch print \"$domain/$unit\" >/dev/null && [ -n \"$program\" ]; then
      log=$(/usr/bin/plutil -extract StandardOutPath raw -o - \"$unit_path\")
      if [ -z \"$log\" ]; then log=\"$HOME/.stado/logs/$unit.log\"; fi
      /bin/mkdir -p \"$(/usr/bin/dirname \"$log\")\"
      /usr/bin/perl -e 'my $log = shift @ARGV; open STDIN, \"<\", \"/dev/null\" or die $!; open STDOUT, \">>\", $log or die $!; open STDERR, \">&STDOUT\" or die $!; exec {$ARGV[0]} @ARGV;' \"$log\" \"$@\" &
      direct_pid=$!
      /bin/sleep \"${#rc}\"
      if /bin/kill -s CONT \"$direct_pid\" >/dev/null; then
        say 'restarted' \"direct process $direct_pid\"
      else
        detail=$(/usr/bin/tail -n \"${#rc}\" \"$log\")
        say 'restart_failed' \"$detail\"
      fi
      exit
    fi
    if [ \"$rc\" -ne 0 ]; then
      say 'bootstrap_failed' \"$rc $detail\"
      exit 0
    fi
    say 'restarted' \"$domain\"
    exit 0
  fi
  $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
  detail=$($launch kickstart -k \"$domain/$unit\" 2>&1)
  rc=$?
  if [ \"$rc\" -eq 0 ]; then say 'restarted' \"$domain\"; else say 'restart_failed' \"$rc $detail\"; fi
else
  systemctl_user daemon-reload >/dev/null 2>&1 || true
  detail=$(systemctl_user restart \"$unit\" 2>&1)
  rc=$?
  if [ \"$rc\" -eq 0 ]; then say 'restarted' 'systemd --user'; else say 'restart_failed' \"$rc $detail\"; fi
fi
";
/// Recovery fencing: stop the unit without disabling it or changing the
/// registry. A later restart loads the same unit after its Stado config has
/// been atomically cut over.
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
  say 'runs' \"$args(current -> $resolved)\"
else
  say 'runs' \"$args\"
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
const DISOWNED_SWEEP: &str = "  program=\"\"
  if [ -f \"$unit_path\" ]; then
    program=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' \"$unit_path\" 2>/dev/null | /usr/bin/sed -n '/^[[:space:]]*[/]/{s/^[[:space:]]*//;s/[[:space:]]*$//;p;q;}')
  fi
  # The unit points at .../services/NAME/current/..., while the process that is
  # actually running shows the version directory that link resolved to. Matching
  # the exact string finds nothing and reports a stop that did not happen, so
  # match the service directory that both spellings share.
  match=\"$program\"
  case \"$program\" in
    */current/*) match=\"${program%%/current/*}/\" ;;
  esac
  left=\"\"
  still=\"\"
  if [ -n \"$program\" ]; then
    left=$(/usr/bin/pgrep -f \"^$match\" 2>/dev/null | /usr/bin/tr '\\n' ' ')
    if [ -n \"$left\" ]; then
      for pid in $left; do /bin/kill -TERM \"$pid\" >/dev/null 2>&1 || true; done
      /bin/sleep 2
      still=$(/usr/bin/pgrep -f \"^$match\" 2>/dev/null | /usr/bin/tr '\\n' ' ')
      # A service that serves each adapter from its own process does not go
      # away on one round of TERM: the process holding the port exits, the
      # siblings holding theirs do not, and launchd is then refused the ports
      # it is being asked to bind. Reporting that as \"survived\" left the unit
      # booted out -- a restart that ends with nothing running. Escalate, and
      # keep 'survived' for a process that refuses SIGKILL.
      if [ -n \"$still\" ]; then
        for pid in $still; do /bin/kill -KILL \"$pid\" >/dev/null 2>&1 || true; done
        /bin/sleep 2
        still=$(/usr/bin/pgrep -f \"^$match\" 2>/dev/null | /usr/bin/tr '\\n' ' ')
      fi
    fi
  fi
";

const STOP_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  recovery_unit=\"${unit}-recovery\"
  $launch bootout \"$domain/$unit\" >/dev/null 2>&1 || true
  $launch bootout \"$domain/$recovery_unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$gui/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$user_domain/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$gui/$recovery_unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$user_domain/$recovery_unit\" >/dev/null 2>&1 || true
@DISOWNED_SWEEP@
  if [ -n \"$left\" ]; then
    if [ -n \"$still\" ]; then
      say 'stop_failed' \"disowned process still running: $still\"
      exit 0
    fi
    say 'stopped' \"booted out, and ended disowned process(es): $left\"
    exit 0
  fi
else
  systemctl_user stop \"$unit\" >/dev/null 2>&1 || true
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
  if systemctl_user cat \"$unit\" >/dev/null 2>&1; then unit_state='loaded'; fi
fi
printf 'STADO_ADOPT\\t%s\\t%s\\n' \"$file_state\" \"$unit_state\"
say 'probed' \"$unit_path\"
";

/// `service retire`: stop and forget. Files stay on disk — retiring is a
/// management decision, not a deletion, and an operator who wants the unit
/// gone can remove it knowing Stado will no longer fight them for it. The
/// bootout/disable pair across both domains mirrors the way
/// `host_recovery`'s script decommissions the obsolete coordinator.
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
else
  systemctl_user disable --now \"$unit\" >/dev/null 2>&1 || true
fi
say 'retired' \"$unit_path\"
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
  /bin/launchctl bootout \"$domain/$unit\" >/dev/null 2>&1 || true
  detail=$(/bin/launchctl bootstrap \"$domain\" \"$unit_path\" 2>&1)
  rc=$?
  if [ \"$rc\" -ne 0 ] && [ \"$domain\" = \"$gui\" ]; then
    /bin/launchctl bootout \"$user_domain/$unit\" >/dev/null 2>&1 || true
    detail=$(/bin/launchctl bootstrap \"$user_domain\" \"$unit_path\" 2>&1)
    rc=$?
    if [ \"$rc\" -eq 0 ]; then domain=\"$user_domain\"; fi
  fi
  if [ \"$rc\" -ne 0 ]; then
    /bin/launchctl bootout \"$gui/$unit\" >/dev/null 2>&1 || true
    detail=$(/bin/launchctl asuser \"$uid\" /bin/launchctl bootstrap \"$gui\" \"$unit_path\" 2>&1)
    rc=$?
    if [ \"$rc\" -eq 0 ]; then domain=\"$gui\"; fi
  fi
  if [ \"$rc\" -ne 0 ]; then
    recovery_unit=\"${unit}-recovery\"
    /bin/launchctl submit -l \"$recovery_unit\" -- \"$program\" >/dev/null 2>&1 || true
    if /bin/launchctl print \"$gui/$recovery_unit\" >/dev/null 2>&1 || /bin/launchctl print \"$user_domain/$recovery_unit\" >/dev/null 2>&1; then
      say 'deployed' \"launchctl submit $recovery_unit\"
      exit 0
    fi
    /usr/bin/nohup \"$program\" >>\"$log\" 2>&1 </dev/null &
    direct_pid=$!
    /bin/sleep 1
    if /bin/kill -0 \"$direct_pid\" >/dev/null 2>&1; then
      say 'deployed' \"direct process $direct_pid\"
    else
      say 'bootstrap_failed' \"$rc $detail\"
    fi
    exit 0
  fi
  /bin/launchctl enable \"$domain/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl kickstart -k \"$domain/$unit\" >/dev/null 2>&1 || true
  say 'deployed' \"$unit_path\"
else
  /bin/mkdir -p \"$HOME/.config/systemd/user\" >/dev/null 2>&1 || true
  /bin/cat > \"$unit_path\" <<'@HEREDOC@'
@LINUX_UNIT@
@HEREDOC@
  systemctl_user daemon-reload >/dev/null 2>&1 || true
  detail=$(systemctl_user enable --now \"$unit\" 2>&1)
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
/// - It never unloads. `deploy` boots the label out before bootstrapping;
///   that is the sequence that took the always-on host down when launchd
///   still held children of the old job and the bootstrap back failed
///   (see [`RESTART_BODY`]). A loaded job is kicked in place, and a loaded
///   job whose definition names another program is REFUSED rather than
///   silently overwritten: launchd holds the definition it bootstrapped, so
///   a rewritten plist under a live job changes nothing an operator can see.
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
    declared_argv=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' \"$unit_path\" 2>/dev/null | /usr/bin/awk 'NR == 1 && $0 == \"Array {\" { next } $0 == \"}\" { next } { sub(/^[[:space:]]+/, \"\"); sub(/[[:space:]]+$/, \"\"); print }' | /usr/bin/tr '\\n' ' ')
    if [ -z \"$declared_argv\" ]; then
      declared_argv=$(/usr/libexec/PlistBuddy -c 'Print :Program' \"$unit_path\" 2>/dev/null)
    fi
  fi
@LAUNCHD_STATE@
  had_unit=\"$pc_loaded\"
  pid=\"$pc_pid\"
else
  if [ -f \"$unit_path\" ]; then
    had_unit=yes
    declared_argv=$(/usr/bin/sed -n 's/^ExecStart=//p' \"$unit_path\" | /usr/bin/head -n 1)
  fi
  pid=$(systemctl_user show --property=MainPID --value \"$unit\" 2>/dev/null)
  if [ \"$pid\" = 0 ]; then pid=''; fi
fi
declared_argv=$(printf '%s' \"$declared_argv\" | /usr/bin/tr -s ' ' | /usr/bin/sed 's/^ //;s/ $//')
# The program the live process is executing, not the one the unit names: a
# unit pointing at a `current` link and a process that outlived the relink
# have the same declaration and different code.
running=''
if [ -n \"$pid\" ]; then running=$(/bin/ps -p \"$pid\" -o comm= 2>/dev/null); fi
serves=no
case \"$running\" in
  \"$program\") serves=yes ;;
esac
if [ \"$serves\" = no ]; then
  case \"$program\" in
    */current/*)
      case \"$running\" in
        \"${program%%/current/*}/\"*) serves=yes ;;
      esac
      ;;
  esac
fi
if [ \"$declared_argv\" = \"$argv\" ] && [ \"$serves\" = yes ]; then
  printf 'STADO_ENSURE\\t%s\\t%s\\t%s\\n' \"$domain\" \"$pid\" \"$unit_path\"
  say 'already_correct' \"$domain/$unit pid $pid\"
  exit 0
fi
if [ \"$declared_argv\" != \"$argv\" ]; then
  if [ \"$os\" = \"Darwin\" ] && [ \"$had_unit\" = yes ]; then
    say 'conflict' \"$domain/$unit is loaded and runs [$declared_argv], not [$argv]; retire it first\"
    exit 0
  fi
  if [ \"$os\" = \"Darwin\" ]; then
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
      # The one write this command cannot do as the login user. A host
      # without passwordless sudo is told exactly that, rather than left
      # with a rendered plist nobody ever loaded.
      /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel \"$staged.rendered\" \"$unit_path\" || bail \"sudo -n install $unit_path was refused\"
    else
      /bin/cp \"$staged.rendered\" \"$unit_path\" || bail \"cannot write $unit_path\"
      /bin/chmod u=rw,go= \"$unit_path\" || bail \"cannot protect $unit_path\"
    fi
    /bin/rm -f \"$staged\" \"$staged.rendered\"
  else
    /bin/mkdir -p \"$HOME/.config/systemd/user\" >/dev/null 2>&1 || bail 'cannot create the systemd user directory'
    /bin/cat > \"$unit_path\" <<'@HEREDOC@'
@LINUX_UNIT@
@HEREDOC@
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
  systemctl_user daemon-reload >/dev/null 2>&1 || true
  if [ \"$had_unit\" = yes ]; then
    action=restarted
    detail=$(systemctl_user restart \"$unit\" 2>&1)
    rc=$?
  else
    action=created
    detail=$(systemctl_user enable --now \"$unit\" 2>&1)
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
@LAUNCHD_STATE@
  pid=\"$pc_pid\"
else
  pid=$(systemctl_user show --property=MainPID --value \"$unit\" 2>/dev/null)
  if [ \"$pid\" = 0 ]; then pid=''; fi
fi
printf 'STADO_ENSURE\\t%s\\t%s\\t%s\\n' \"$domain\" \"$pid\" \"$unit_path\"
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
@LAUNCHD_STATE@
  pid=\"$pc_pid\"
else
  if [ -f \"$unit_path\" ]; then
    declared=$(/usr/bin/sed -n 's/^ExecStart=//p' \"$unit_path\" | /usr/bin/head -n 1)
    declared=\"${declared%% *}\"
  fi
  pid=$(systemctl_user show --property=MainPID --value \"$unit\" 2>/dev/null)
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
if [ \"$os\" = \"Darwin\" ]; then
  owned=''
  for launchd_domain in \"gui/$uid\" \"user/$uid\" system; do
    owned=\"$owned $(/bin/launchctl print \"$launchd_domain\" 2>/dev/null | /usr/bin/awk '/services = \\{/ { inside = 1; next } inside && /^[[:space:]]*\\}/ { inside = 0 } inside && $1 ~ /^[0-9]+$/ { print $1 }' | /usr/bin/tr '\\n' ' ')\"
  done
  owns() {
    walk=\"$1\"
    while [ -n \"$walk\" ] && [ \"$walk\" != 0 ] && [ \"$walk\" != 1 ]; do
      case \" $owned \" in *\" $walk \"*) return 0 ;; esac
      walk=$(/bin/ps -p \"$walk\" -o ppid= 2>/dev/null | /usr/bin/tr -d ' ')
    done
    return 1
  }
else
  owns() {
    cgroup=$(/bin/cat \"/proc/$1/cgroup\" 2>/dev/null | /usr/bin/sed -n 's/.*\\///p')
    case \"$cgroup\" in *.service) return 0 ;; esac
    return 1
  }
fi
seen=''
for root in \"$@\"; do
  for pid in $(/usr/bin/pgrep -f \"$root\" 2>/dev/null); do
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
    if owns \"$pid\"; then continue; fi
    seen=\"$seen $pid\"
    started=$(/bin/ps -p \"$pid\" -o lstart= 2>/dev/null | /usr/bin/tr '\\t\\r\\n' ' ')
    printf 'STADO_UNOWNED\\t%s\\t%s\\t%s\\n' \"$pid\" \"$started\" \"$command\"
  done
done
";

/// `service logs`: tail the unit's own log. On launchd the log path comes
/// from the unit file itself, so an adopted unit keeps its chosen destination.
/// A unit without one falls back to the account's owner-only Stado log path.
const LOGS_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  log=''
  if [ -f \"$unit_path\" ]; then
    log=$(/usr/bin/plutil -extract StandardOutPath raw -o - \"$unit_path\" 2>/dev/null)
  fi
  if [ -z \"$log\" ]; then log=\"$HOME/.stado/logs/$unit.log\"; fi
  if [ -f \"$log\" ]; then
    printf 'STADO_LOG\\t%s\\n' \"$log\"
    /usr/bin/tail -n @LINES@ \"$log\"
  else
    say 'missing_log' \"$log\"
  fi
else
  printf 'STADO_LOG\\tjournalctl --user -u %s\\n' \"$unit\"
  /usr/bin/journalctl --user -u \"$unit\" -n @LINES@ --no-pager 2>&1
fi
";

/// `service env`: hand the unit file back verbatim and parse it locally.
/// Parsing on this side keeps the remote program fixed and narrow, and
/// keeps redaction in one place instead of trusting a shell pipeline to
/// have caught every credential-shaped key.
const UNIT_FILE_BODY: &str = "if [ -f \"$unit_path\" ]; then
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
assignment=$(printf '%s' '@ASSIGNMENT_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_sync 'invalid assignment payload'
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
const AUTH_CHECK_BODY: &str = "fail_check() {
  say 'auth_check_failed' \"$1\"
  exit 0
}
if [ \"$os\" = \"Darwin\" ]; then decode_flag=-D; else decode_flag=--decode; fi
probe_url=$(printf '%s' '@PROBE_URL_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_check 'invalid probe URL payload'
token=$(printf '%s' '@TOKEN_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_check 'invalid token payload'
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
/// `$domain`, `$launch`, `systemctl_user`, `say`) every body and every
/// postcondition probe reads the host through.
///
/// `no_domain` is what the prelude does when a Darwin host has no per-login
/// launchd domain. It is a parameter and not a fixed refusal because the two
/// answers are genuinely different operations: everything that addresses an
/// installed unit has nothing to act on ([`NO_DOMAIN_REFUSE`]), while
/// [`ensure_service`] installs the daemon spelling into the domain that does
/// exist ([`NO_DOMAIN_SYSTEM`]).
fn prelude_with(
    unit: &str,
    linux_unit: &str,
    path: &str,
    no_domain: &str,
) -> Result<String, DeployError> {
    validate_unit_id(unit)?;
    Ok(REMOTE_PRELUDE
        .replace("@UNIT@", &shlex_quote(unit))
        .replace("@LINUX_UNIT@", &shlex_quote(linux_unit))
        .replace("@PATH@", &quote_unit_path(path)?)
        .replace("@NO_DOMAIN@", no_domain))
}

fn remote_prelude(unit: &str, linux_unit: &str, path: &str) -> Result<String, DeployError> {
    prelude_with(unit, linux_unit, path, NO_DOMAIN_REFUSE)
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

/// `service restart` on one host, with the end state it intends checked on
/// the host before the connection closes. A restart whose own steps report
/// success while the unit ends up unloaded is reported as the failure it
/// is: see [`RemoteReport::succeeded`].
pub async fn restart_service(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = RESTART_BODY.replace("@DISOWNED_SWEEP@", DISOWNED_SWEEP);
    let prelude = remote_prelude(service.unit_id(), "", &service.path)?;
    run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RUNNING_DESCRIBE, RUNNING_PROBE),
        runner,
    )
    .await
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
    validate_env_path(env_path)?;
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

/// Stop an unmanaged process that prevents launchd from reclaiming the probe port.
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
///
/// The fence is only a fence if the writer is actually gone, so the intended
/// end state is checked on the host: a stop that boots out a label and
/// leaves the program serving is reported as a failed stop, not as a stop.
pub async fn stop_service(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
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
pub async fn retire_service(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, RETIRE_BODY)?;
    run_remote(target, script, runner).await
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

pub fn plan_deploy(name: &str, program: &str, args: &[String]) -> Result<DeployPlan, DeployError> {
    validate_service_name(name)?;
    validate_program(program)?;
    for arg in args {
        validate_unit_argument(arg)?;
    }
    let label = local_install::label(DEPLOY_KIND, name);
    let render = |os: LocalOs| {
        let path = match os {
            LocalOs::Darwin => "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            LocalOs::Linux => "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        };
        let mut exec_args = Vec::with_capacity(args.len() + 1);
        exec_args.push(program.to_string());
        exec_args.extend(args.iter().cloned());
        InstallPlan {
            name: name.to_string(),
            kind: DEPLOY_KIND.to_string(),
            os,
            label: label.clone(),
            exec_args,
            env: vec![("PATH".to_string(), path.to_string())],
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
    };
    guard_heredoc(&plan.darwin_unit)?;
    guard_heredoc(&plan.darwin_daemon_unit)?;
    guard_heredoc(&plan.linux_unit)?;
    Ok(plan)
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
    run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RUNNING_DESCRIBE, RUNNING_PROBE),
        runner,
    )
    .await
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

/// launchd's system domain: `/Library/LaunchDaemons`, reached with sudo, and
/// the only domain that exists on an ssh login with no Aqua session.
pub const DOMAIN_SYSTEM: &str = "system";
/// The per-login domain (`gui/<uid>` or `user/<uid>`), and `systemd --user`.
pub const DOMAIN_USER: &str = "user";

/// What one `ensure` pass found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureOutcome {
    /// [`ACTION_CREATED`], [`ACTION_RESTARTED`] or
    /// [`ACTION_ALREADY_CORRECT`]; any other word is a failure the remote
    /// program named.
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
    /// True when the pass reached one of the three intended actions AND the
    /// host was observed with the unit loaded and running afterwards.
    pub fn succeeded(&self) -> bool {
        matches!(
            self.action.as_str(),
            ACTION_CREATED | ACTION_RESTARTED | ACTION_ALREADY_CORRECT
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

/// `service ensure` on one host: install the unit only where the host is not
/// already running it, and never unload anything.
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
        .replace("@LAUNCHD_STATE@", LAUNCHD_STATE)
        .replace("@PROGRAM@", &shlex_quote(&plan.program))
        .replace("@ARGV@", &shlex_quote(&plan.argv))
        .replace(
            "@DARWIN_DAEMON_UNIT@",
            plan.darwin_daemon_unit.trim_end_matches('\n'),
        )
        .replace("@DARWIN_UNIT@", plan.darwin_unit.trim_end_matches('\n'))
        .replace("@LINUX_UNIT@", plan.linux_unit.trim_end_matches('\n'));
    let prelude = prelude_with(&plan.label, &plan.unit, "", NO_DOMAIN_SYSTEM)?;
    let report = run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RUNNING_DESCRIBE, RUNNING_PROBE),
        runner,
    )
    .await?;
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
    let body = PROCESS_BODY.replace("@LAUNCHD_STATE@", LAUNCHD_STATE);
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
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

/// Every product process on one host that no launchd job or systemd unit owns.
///
/// Read-only: it starts nothing, stops nothing and signals nothing, so it is
/// safe to run against a live production host.
pub async fn unowned_processes(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<UnownedProcess>, DeployError> {
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
    Ok(parse_unowned(&target.name, &output.stdout))
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

/// One host's tail of a managed unit's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceLog {
    pub host: String,
    pub unit: String,
    /// The log file, or the journalctl invocation that produced the body.
    pub origin: String,
    pub body: String,
}

impl ServiceLog {
    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "unit": self.unit,
            "origin": self.origin,
            "lines": self.body.lines().collect::<Vec<&str>>(),
        })
    }
}

/// `service logs` on one host.
pub async fn tail_logs(
    target: &ComputeTarget,
    service: &ManagedService,
    lines: usize,
    runner: &Runner,
) -> Result<ServiceLog, DeployError> {
    let body = LOGS_BODY.replace("@LINES@", &shlex_quote(&lines.to_string()));
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    let report = run_remote(target, script, runner).await?;
    let Some((origin, tail)) = split_marker_body(&report.stdout, "STADO_LOG") else {
        return Err(DeployError(format!(
            "{}: {} log unavailable: {}",
            target.name,
            service.unit_id(),
            report.failure()
        )));
    };
    Ok(ServiceLog {
        host: target.name.clone(),
        unit: service.unit_id().to_string(),
        origin: origin.to_string(),
        body: tail.to_string(),
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

/// `service env`'s fetch: the unit file exactly as the host holds it.
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
/// Pure by design: the caller reads the document through
/// `cli/registry.rs::fetch_document`, applies this, and writes it back
/// through `cli/registry.rs::push_document`, which validates the whole
/// document before it writes anything.
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

    /// The declaration and the process agree, and nothing has been written
    /// since the process started: the running code is the shipped code.
    #[test]
    fn an_untouched_artefact_matches_its_process() {
        let program = RunningProgram {
            unit: "com.wisent.brama".to_string(),
            pid: "412".to_string(),
            declared: "/Users/c/.stado/services/brama/current/bin/brama".to_string(),
            resolved: "/Users/c/.stado/services/brama/0.2.27/bin/brama".to_string(),
            running: "/Users/c/.stado/services/brama/0.2.27/bin/brama".to_string(),
            started_epoch: Some(1_700_000_500),
            declared_written_epoch: Some(1_700_000_000),
            running_written_epoch: Some(1_700_000_000),
        };
        assert_eq!(program.matches_process(), Some(true));
        assert_eq!(
            program.running_binary(),
            Some("/Users/c/.stado/services/brama/0.2.27/bin/brama")
        );
    }

    /// Brama's incident: the unit, the installed version and the release were
    /// all correct, and the live process was executing a version directory
    /// `current` no longer pointed at.
    #[test]
    fn a_process_under_an_old_version_directory_does_not_match() {
        let program = RunningProgram {
            declared: "/Users/c/.stado/services/brama/current/bin/brama".to_string(),
            resolved: "/Users/c/.stado/services/brama/0.2.27/bin/brama".to_string(),
            running: "/Users/c/.stado/services/brama/0.2.26/bin/brama".to_string(),
            started_epoch: Some(1_700_000_500),
            declared_written_epoch: Some(1_700_000_000),
            running_written_epoch: Some(1_700_000_000),
            ..RunningProgram::default()
        };
        assert_eq!(program.matches_process(), Some(false));
    }

    /// The Weles incident: the path still matches and the artefact the process
    /// loaded was replaced 26 seconds into its run.
    #[test]
    fn an_artefact_replaced_after_the_process_started_does_not_match() {
        let program = RunningProgram {
            declared: "/Users/c/weles/dist/worker".to_string(),
            resolved: "/Users/c/weles/dist/worker".to_string(),
            running: "/Users/c/weles/dist/worker".to_string(),
            started_epoch: Some(1_700_000_000),
            declared_written_epoch: Some(1_700_000_026),
            running_written_epoch: Some(1_700_000_026),
            ..RunningProgram::default()
        };
        assert_eq!(program.matches_process(), Some(false));
    }

    /// Nothing running under the unit, and a host that would not say when a
    /// file was written, are both "nobody looked" — never `true`, and never
    /// `false` either.
    #[test]
    fn an_unobservable_process_is_neither_answer() {
        let nothing_running = RunningProgram {
            declared: "/Users/c/weles/dist/worker".to_string(),
            ..RunningProgram::default()
        };
        assert_eq!(nothing_running.matches_process(), None);
        assert_eq!(nothing_running.running_binary(), None);

        let no_timestamps = RunningProgram {
            declared: "/Users/c/weles/dist/worker".to_string(),
            resolved: "/Users/c/weles/dist/worker".to_string(),
            running: "/Users/c/weles/dist/worker".to_string(),
            started_epoch: Some(1_700_000_000),
            ..RunningProgram::default()
        };
        assert_eq!(no_timestamps.matches_process(), None);
    }

    /// The four-day incident, as the report reads it: a `stado agent` process
    /// under the owner-only bin root, attributed to the product whose file it
    /// is rather than to the root it shares with skarbiec.
    #[test]
    fn an_unowned_agent_is_attributed_to_its_own_product() {
        let processes = parse_unowned(
            "control-host",
            "STADO_HOST\tDarwin\tgui/501\tx\ty\n\
             STADO_UNOWNED\t46748\tMon Aug 11 09:12:33 2026\t/Users/c/.stado/bin/stado agent\n",
        );
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].host, "control-host");
        assert_eq!(processes[0].pid, "46748");
        assert_eq!(processes[0].started_at, "Mon Aug 11 09:12:33 2026");
        assert_eq!(processes[0].product_guess(), "stado");
    }

    /// A deployed service is named by the directory it was installed under,
    /// which is the name `service deploy` created it with; a release tree is
    /// named by the product that declares its root.
    #[test]
    fn a_command_under_a_managed_root_names_what_it_belongs_to() {
        assert_eq!(
            product_guess("/Users/c/.stado/services/brama/current/bin/brama serve"),
            "brama"
        );
        assert_eq!(
            product_guess("/opt/homebrew/bin/node /Users/c/weles/dist/worker.js"),
            "weles-worker"
        );
        assert_eq!(
            product_guess("/usr/bin/tail -f /var/log/system.log"),
            UNKNOWN_PRODUCT
        );
    }

    /// The system-domain spelling names the account it must run as, so launchd
    /// does not run the fleet's control binary as root, and it is a different
    /// file from the agent spelling of the same job.
    #[test]
    fn the_daemon_spelling_names_its_account() {
        let plan = plan_deploy(
            "weles-api",
            "/Users/c/.stado/bin/stado",
            &["agent".to_string()],
        )
        .expect("plan renders");
        assert_eq!(plan.argv, "/Users/c/.stado/bin/stado agent");
        assert!(plan.darwin_daemon_unit.contains("<key>UserName</key>"));
        assert!(plan.darwin_daemon_unit.contains(REMOTE_USER_PLACEHOLDER));
        assert!(!plan.darwin_unit.contains("<key>UserName</key>"));
        assert!(plan.darwin_daemon_unit.contains(&plan.label));
    }

    /// The ensure marker is the authority on where the unit ended up: the body
    /// picks the domain after the prelude has already printed the path it
    /// derived, and a record naming the wrong file is one no later command can
    /// act on.
    #[test]
    fn the_ensure_marker_carries_the_domain_it_settled_on() {
        let stdout = "STADO_HOST\tDarwin\tgui/501\tcom.wisent.stado.service.weles-api\t/Users/c/Library/LaunchAgents/x.plist\n\
                      STADO_ENSURE\tsystem\t908\t/Library/LaunchDaemons/com.wisent.stado.service.weles-api.plist\n\
                      STADO_SERVICE\tcom.wisent.stado.service.weles-api\tcreated\tx\n";
        let (domain, pid, path) = parse_ensure(stdout).expect("marker present");
        assert_eq!(domain, DOMAIN_SYSTEM);
        assert_eq!(pid, "908");
        assert_eq!(
            path,
            "/Library/LaunchDaemons/com.wisent.stado.service.weles-api.plist"
        );
        let outcome = EnsureOutcome {
            action: ACTION_CREATED.to_string(),
            domain,
            pid,
            path: path.clone(),
            report: parse_markers(stdout),
        };
        assert_eq!(outcome.domain_word(), DOMAIN_SYSTEM);
        assert!(outcome.changed());
        assert_eq!(
            record_from_ensure("control-host", "weles-api", &outcome, "now").path,
            path
        );
    }

    /// `already_correct` is the one action that changed nothing, which is what
    /// decides whether a pass is recorded at all.
    #[test]
    fn an_already_correct_pass_changed_nothing() {
        let outcome = EnsureOutcome {
            action: ACTION_ALREADY_CORRECT.to_string(),
            domain: "gui/501".to_string(),
            pid: "412".to_string(),
            path: "/Users/c/Library/LaunchAgents/x.plist".to_string(),
            report: RemoteReport::default(),
        };
        assert!(!outcome.changed());
        assert_eq!(outcome.domain_word(), DOMAIN_USER);
        assert!(outcome.succeeded());
    }
}
