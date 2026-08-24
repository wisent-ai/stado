//! `stado placement move` — one fenced transaction for a colocated service
//! group — and `stado host publish-placement-policy`, which makes the registry
//! the only writer of a host's Weles placement policy.
//!
//! The registry profile is the complete operational contract: concrete units
//! per host, stop/start order, durable files, loopback health probes, and routing
//! units. The command claims the profile through registry CAS, fences the source,
//! copies state only after writers stop, activates and probes the destination,
//! then commits the service declarations with a second CAS. Every failure before
//! that commit restores destination files, routing, and source services.
//!
//! Both halves answer the same question — which host may do what — and the
//! second half exists because one part of that question was answered twice. A
//! service's placement lives in the registry and moves under transaction; a
//! worker's placement lived in the registry AND in a file on the worker's own
//! disk, and only the file decided.

use std::collections::BTreeSet;
use std::process::Stdio;
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{SecondsFormat, Utc};
use clap::Subcommand;
use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::deploy::service::{self, ManagedService, SOURCE_REGISTRY};
use crate::deploy::{host_channel, production_runner, CommandOutput, DeployError, Runner};
use crate::placement::{
    self, PlacementHost, PlacementProfile, PlacementState, PlacementTransaction, PlacementUnit,
};
use crate::targets::{self, ComputeTarget, Registry, WelesPolicy};

use super::{registry, CmdError};

type RegistryCommitter =
    Arc<dyn Fn(Value, String) -> BoxFuture<'static, Result<String, CmdError>> + Send + Sync>;

fn production_committer() -> RegistryCommitter {
    Arc::new(|document, expected_generation| {
        Box::pin(async move { registry::push_document_if(&document, &expected_generation).await })
    })
}

#[derive(Subcommand)]
pub enum PlacementCommands {
    /// Relocate one complete service group to another registered host.
    Move {
        /// Logical services naming one exact registry placement profile.
        #[arg(required = true, num_args = 1..)]
        services: Vec<String>,
        /// Registered destination host.
        #[arg(long)]
        to_host: String,
        /// Emit the committed transaction report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop an instance running where the directory places nothing.
    ///
    /// `doctor`'s placement row tells the operator to end exactly this, and
    /// until now no command could: `service stop` refuses a host that
    /// declares no such unit, which is the definition of the squatter it is
    /// asked to remove. The port comes from the directory, so an instance can
    /// only be evicted from a host the directory does NOT place it on.
    Evict {
        /// Logical service the directory declares.
        service: String,
        /// Registered host holding the port it should not hold.
        #[arg(long)]
        host: String,
        /// Emit the eviction report as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: PlacementCommands) -> Result<(), CmdError> {
    match command {
        PlacementCommands::Move {
            services,
            to_host,
            json,
        } => move_services(&services, &to_host, json).await,
        PlacementCommands::Evict {
            service,
            host,
            json,
        } => evict(&service, &host, json).await,
    }
}

/// Stop the process holding a declared service's port on a host the directory
/// does not place it on.
///
/// The guard is the directory itself: eviction is refused on the host that
/// holds the placement, so the command can never take down the real instance.
/// The remote step is the same listener reset launchd already needs when an
/// unmanaged fallback keeps the port.
async fn evict(service: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let entry = document
        .get("service_directory")
        .and_then(|block| block.get("services"))
        .and_then(|services| services.get(service))
        .ok_or_else(|| {
            CmdError::click(format!(
                "the directory declares no service named {service:?}"
            ))
        })?;
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click(format!("{service} has no active_host in the directory")))?;
    if active.starts_with(host) || host.starts_with(active) {
        return Err(CmdError::click(format!(
            "{service} is placed on {active}; evicting its own host would end the real instance. \
             Use `stado service stop {service} --host {host}` to stop a placed service"
        )));
    }
    let port = super::directory::service_port(entry, active)
        .ok_or_else(|| CmdError::click(format!("the directory declares no port for {service}")))?;
    let target = host_channel::canonical_target(host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // The listener reset takes a probe URL and reads its port; the unit id and
    // path are only markers here, because the squatter has no unit on this
    // host - that is what makes it a squatter.
    let squatter = service::launchd_service(
        host,
        &format!("unmanaged.{service}"),
        "",
        SOURCE_REGISTRY,
        &Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
    );
    let report = service::reset_service_listener(
        &target,
        &squatter,
        &format!("http://127.0.0.1:{port}/"),
        &production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": service,
                "host": host,
                "placed_on": active,
                "port": port,
                "status": report.status,
                "detail": report.detail,
            }))?
        );
    } else {
        println!(
            "{host}\t{service}\tport {port}\t{}\t{}",
            report.status, report.detail
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct StateSnapshot {
    spec: PlacementState,
    bytes: Option<Vec<u8>>,
}

#[derive(Default)]
struct Progress {
    source_stopped: bool,
    destination_written: Vec<String>,
    route_applied: bool,
    destination_started: bool,
    source_retired: bool,
}

struct MoveContext {
    profile: PlacementProfile,
    source: ComputeTarget,
    destination: ComputeTarget,
    registry: Registry,
    claimed_document: Value,
    claim_generation: String,
    transaction: PlacementTransaction,
}

fn deploy_error(error: DeployError) -> CmdError {
    CmdError::click(error.to_string())
}

fn parse_registry(document: &Value) -> Result<Registry, CmdError> {
    let text = serde_json::to_string(document)?;
    targets::load_registry_from_str(&text).map_err(|error| CmdError::click(error.to_string()))
}

fn target<'a>(registry: &'a Registry, name: &str) -> Result<&'a ComputeTarget, CmdError> {
    host_channel::resolve_target(registry, name).map_err(deploy_error)
}

fn declared_profile_hosts(
    registry: &Registry,
    profile: &PlacementProfile,
) -> Result<Vec<String>, CmdError> {
    let mut complete = Vec::new();
    let mut partial = Vec::new();
    for (host, host_profile) in &profile.hosts {
        let target = target(registry, host)?;
        let declared: Vec<ManagedService> = service::declared_services(target)
            .into_iter()
            .filter(|managed| managed.source == SOURCE_REGISTRY)
            .collect();
        let matched = host_profile
            .units
            .values()
            .filter(|spec| {
                declared
                    .iter()
                    .any(|managed| managed.matches(&spec.unit) || managed.matches(&spec.name))
            })
            .count();
        if matched == profile.services.len() {
            complete.push(host.clone());
        } else if matched != 0 {
            partial.push(format!("{host} ({matched}/{})", profile.services.len()));
        }
    }
    if !partial.is_empty() {
        return Err(CmdError::click(format!(
            "placement profile {:?} is split or incomplete: {}",
            profile.name,
            partial.join(", ")
        )));
    }
    Ok(complete)
}

fn profile_host<'a>(
    profile: &'a PlacementProfile,
    host: &str,
) -> Result<&'a PlacementHost, CmdError> {
    profile.hosts.get(host).ok_or_else(|| {
        CmdError::click(format!(
            "placement profile {:?} does not support destination {:?}",
            profile.name, host
        ))
    })
}

fn unit<'a>(host: &'a PlacementHost, logical: &str) -> Result<&'a PlacementUnit, CmdError> {
    host.units.get(logical).ok_or_else(|| {
        CmdError::click(format!(
            "placement profile has no concrete unit for service {logical:?}"
        ))
    })
}

fn marker_line<'a>(output: &'a CommandOutput, marker: &str) -> Option<&'a str> {
    output.stdout.lines().find(|line| line.starts_with(marker))
}

async fn run_host_script(
    target: &ComputeTarget,
    script: &str,
    runner: &Runner,
    operation: &str,
) -> Result<CommandOutput, CmdError> {
    let output = host_channel::run_script(target, script, runner)
        .await
        .map_err(deploy_error)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: {operation} failed: {}",
            target.name,
            host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    Ok(output)
}

fn unit_script_head(spec: &PlacementUnit) -> String {
    let unit = STANDARD.encode(spec.unit.as_bytes());
    let path = STANDARD.encode(spec.path.as_bytes());
    let kind = STANDARD.encode(spec.kind.as_bytes());
    format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
unit=$(printf '%s' '{unit}' | /usr/bin/base64 "$decode")
unit_path=$(printf '%s' '{path}' | /usr/bin/base64 "$decode")
expected_kind=$(printf '%s' '{kind}' | /usr/bin/base64 "$decode")
os=$(/usr/bin/uname -s)
uid=$(/usr/bin/id -u)
if [ "$os" = Darwin ]; then
  [ "$expected_kind" = launchd ] || {{ printf 'expected systemd, found Darwin\n' >&2; exit 65; }}
  case "$unit_path" in
    /Library/LaunchDaemons/*) domain=system ;;
    *)
      if /bin/launchctl print "gui/$uid" >/dev/null 2>&1; then domain="gui/$uid"; else domain="user/$uid"; fi
      ;;
  esac
  lc() {{
    if [ "$domain" = system ] && [ "$uid" -ne 0 ]; then /usr/bin/sudo -n /bin/launchctl "$@"; else /bin/launchctl "$@"; fi
  }}
elif [ "$os" = Linux ]; then
  [ "$expected_kind" = systemd ] || {{ printf 'expected launchd, found Linux\n' >&2; exit 65; }}
  systemctl_user() {{ /usr/bin/systemctl --user "$@"; }}
else
  printf 'unsupported OS: %s\n' "$os" >&2
  exit 65
fi
"#
    )
}

#[derive(Debug, Clone, Copy)]
struct UnitStatus {
    present: bool,
    loaded: bool,
}

async fn probe_unit(
    target: &ComputeTarget,
    spec: &PlacementUnit,
    runner: &Runner,
) -> Result<UnitStatus, CmdError> {
    let script = format!(
        "{}{}",
        unit_script_head(spec),
        r#"present=no
loaded=no
if [ -f "$unit_path" ]; then present=yes; fi
if [ "$os" = Darwin ]; then
  if lc print "$domain/$unit" >/dev/null 2>&1; then loaded=yes; fi
else
  if systemctl_user is-active --quiet "$unit"; then loaded=yes; fi
fi
printf 'STADO_PLACEMENT_UNIT\t%s\t%s\n' "$present" "$loaded"
"#
    );
    let output = run_host_script(target, &script, runner, "unit probe").await?;
    let line = marker_line(&output, "STADO_PLACEMENT_UNIT\t").ok_or_else(|| {
        CmdError::click(format!(
            "{}: unit probe returned no placement marker",
            target.name
        ))
    })?;
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 3 {
        return Err(CmdError::click(format!(
            "{}: malformed unit probe marker",
            target.name
        )));
    }
    Ok(UnitStatus {
        present: fields[1] == "yes",
        loaded: fields[2] == "yes",
    })
}

#[derive(Debug, Clone, Copy)]
enum UnitAction {
    Stop,
    Start,
    Retire,
}

impl UnitAction {
    fn name(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Start => "start",
            Self::Retire => "retire",
        }
    }
}

async fn act_on_unit(
    target: &ComputeTarget,
    spec: &PlacementUnit,
    action: UnitAction,
    runner: &Runner,
) -> Result<(), CmdError> {
    let body = match action {
        UnitAction::Stop => {
            r#"if [ "$os" = Darwin ]; then
  lc bootout "$domain/$unit" >/dev/null 2>&1 || true
else
  systemctl_user stop "$unit"
fi
"#
        }
        UnitAction::Start => {
            r#"if [ "$os" = Darwin ]; then
  lc bootout "$domain/$unit" >/dev/null 2>&1 || true
  lc enable "$domain/$unit" >/dev/null
  lc bootstrap "$domain" "$unit_path"
  lc print "$domain/$unit" >/dev/null
else
  systemctl_user daemon-reload
  systemctl_user enable "$unit" >/dev/null
  systemctl_user restart "$unit"
  systemctl_user is-active --quiet "$unit"
fi
"#
        }
        UnitAction::Retire => {
            r#"if [ "$os" = Darwin ]; then
  lc bootout "$domain/$unit" >/dev/null 2>&1 || true
  lc disable "$domain/$unit" >/dev/null
else
  systemctl_user disable --now "$unit"
fi
"#
        }
    };
    let script = format!(
        "{}{}printf 'STADO_PLACEMENT_ACTION\\t{}\\tok\\n'\n",
        unit_script_head(spec),
        body,
        action.name()
    );
    let output = run_host_script(
        target,
        &script,
        runner,
        &format!("{} {}", action.name(), spec.unit),
    )
    .await?;
    if marker_line(
        &output,
        &format!("STADO_PLACEMENT_ACTION\t{}\tok", action.name()),
    )
    .is_none()
    {
        return Err(CmdError::click(format!(
            "{}: {} {} returned no success marker",
            target.name,
            action.name(),
            spec.unit
        )));
    }
    Ok(())
}

fn state_path_payload(path: &str) -> String {
    STANDARD.encode(path.as_bytes())
}

async fn state_exists(
    target: &ComputeTarget,
    state: &PlacementState,
    runner: &Runner,
) -> Result<bool, CmdError> {
    let path = state_path_payload(&state.path);
    let script = format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
relative=$(printf '%s' '{path}' | /usr/bin/base64 "$decode")
full="$HOME/$relative"
if [ -f "$full" ]; then printf 'STADO_PLACEMENT_STATE\tpresent\n';
elif [ -e "$full" ]; then printf '%s is not a regular file\n' "$full" >&2; exit 65;
else printf 'STADO_PLACEMENT_STATE\tmissing\n'; fi
"#
    );
    let output = run_host_script(target, &script, runner, "state preflight").await?;
    Ok(marker_line(&output, "STADO_PLACEMENT_STATE\tpresent").is_some())
}

async fn read_state(
    target: &ComputeTarget,
    state: &PlacementState,
    runner: &Runner,
) -> Result<StateSnapshot, CmdError> {
    let path = state_path_payload(&state.path);
    let script = format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
relative=$(printf '%s' '{path}' | /usr/bin/base64 "$decode")
full="$HOME/$relative"
if [ ! -e "$full" ]; then printf 'STADO_PLACEMENT_STATE\tmissing\n'; exit 0; fi
[ -f "$full" ] || {{ printf '%s is not a regular file\n' "$full" >&2; exit 65; }}
payload=$(/usr/bin/base64 < "$full" | /usr/bin/tr -d '\r\n')
printf 'STADO_PLACEMENT_STATE\tpresent\t%s\n' "$payload"
"#
    );
    let output = run_host_script(target, &script, runner, "state read").await?;
    let line = marker_line(&output, "STADO_PLACEMENT_STATE\t").ok_or_else(|| {
        CmdError::click(format!(
            "{}: state read returned no marker for {}",
            target.name, state.path
        ))
    })?;
    let mut fields = line.splitn(3, '\t');
    let _marker = fields.next();
    match fields.next() {
        Some("missing") if !state.required => Ok(StateSnapshot {
            spec: state.clone(),
            bytes: None,
        }),
        Some("missing") => Err(CmdError::click(format!(
            "{}: required state {} disappeared after fencing",
            target.name, state.path
        ))),
        Some("present") => {
            let payload = fields.next().unwrap_or_default();
            let bytes = STANDARD.decode(payload).map_err(|error| {
                CmdError::click(format!(
                    "{}: invalid state payload for {}: {error}",
                    target.name, state.path
                ))
            })?;
            Ok(StateSnapshot {
                spec: state.clone(),
                bytes: Some(bytes),
            })
        }
        _ => Err(CmdError::click(format!(
            "{}: malformed state marker for {}",
            target.name, state.path
        ))),
    }
}

async fn write_state(
    target: &ComputeTarget,
    snapshot: &StateSnapshot,
    transaction_id: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let path = state_path_payload(&snapshot.spec.path);
    let transaction = STANDARD.encode(transaction_id.as_bytes());
    let (present, payload) = match &snapshot.bytes {
        Some(bytes) => ("yes", STANDARD.encode(bytes)),
        None => ("no", String::new()),
    };
    let script = format!(
        r#"set -eu
umask 077
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
relative=$(printf '%s' '{path}' | /usr/bin/base64 "$decode")
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
full="$HOME/$relative"
backup="$full.pre-stado-placement-$txn"
meta="$backup.meta"
parent=$(/usr/bin/dirname "$full")
/bin/mkdir -p "$parent"
[ ! -e "$backup" ] && [ ! -e "$meta" ] || {{ printf 'placement backup already exists: %s\n' "$backup" >&2; exit 73; }}
had=no
if [ -f "$full" ]; then /bin/cp -p "$full" "$backup"; had=yes;
elif [ -e "$full" ]; then printf '%s is not a regular file\n' "$full" >&2; exit 65; fi
printf '%s\n' "$had" > "$meta"
if [ '{present}' = yes ]; then
  tmp="$full.placement-$txn.tmp"
  trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
  printf '%s' '{payload}' | /usr/bin/base64 "$decode" > "$tmp"
  /bin/chmod 600 "$tmp"
  /bin/mv -f "$tmp" "$full"
else
  /bin/rm -f "$full"
fi
printf 'STADO_PLACEMENT_WRITE\tok\t%s\n' "$had"
"#
    );
    let output = run_host_script(target, &script, runner, "state install").await?;
    if marker_line(&output, "STADO_PLACEMENT_WRITE\tok\t").is_none() {
        return Err(CmdError::click(format!(
            "{}: state install returned no marker for {}",
            target.name, snapshot.spec.path
        )));
    }
    Ok(())
}

async fn restore_state(
    target: &ComputeTarget,
    path: &str,
    transaction_id: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let path_payload = state_path_payload(path);
    let transaction = STANDARD.encode(transaction_id.as_bytes());
    let script = format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
relative=$(printf '%s' '{path_payload}' | /usr/bin/base64 "$decode")
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
full="$HOME/$relative"
backup="$full.pre-stado-placement-$txn"
meta="$backup.meta"
if [ ! -f "$meta" ]; then
  printf 'STADO_PLACEMENT_RESTORE\tok\tuntouched\n'
  exit 0
fi
had=$(/bin/cat "$meta")
if [ "$had" = yes ]; then
  [ -f "$backup" ] || {{ printf 'placement backup is missing: %s\n' "$backup" >&2; exit 74; }}
  /bin/mv -f "$backup" "$full"
else
  /bin/rm -f "$full"
fi
/bin/rm -f "$meta"
printf 'STADO_PLACEMENT_RESTORE\tok\n'
"#
    );
    let output = run_host_script(target, &script, runner, "state rollback").await?;
    if marker_line(&output, "STADO_PLACEMENT_RESTORE\tok").is_none() {
        return Err(CmdError::click(format!(
            "{}: state rollback returned no marker for {path}",
            target.name
        )));
    }
    Ok(())
}

async fn cleanup_state_backup(
    target: &ComputeTarget,
    path: &str,
    transaction_id: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let path_payload = state_path_payload(path);
    let transaction = STANDARD.encode(transaction_id.as_bytes());
    let script = format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
relative=$(printf '%s' '{path_payload}' | /usr/bin/base64 "$decode")
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
/bin/rm -f "$HOME/$relative.pre-stado-placement-$txn" "$HOME/$relative.pre-stado-placement-$txn.meta"
printf 'STADO_PLACEMENT_CLEANUP\tok\n'
"#
    );
    let output = run_host_script(target, &script, runner, "backup cleanup").await?;
    if marker_line(&output, "STADO_PLACEMENT_CLEANUP\tok").is_none() {
        return Err(CmdError::click(format!(
            "{}: backup cleanup returned no marker for {path}",
            target.name
        )));
    }
    Ok(())
}

async fn health_probe(
    target: &ComputeTarget,
    url: &str,
    attempts: usize,
    runner: &Runner,
) -> Result<(), CmdError> {
    let parsed = url::Url::parse(url)
        .map_err(|error| CmdError::click(format!("invalid placement probe URL: {error}")))?;
    let loopback = parsed
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if parsed.scheme() != "http" || !loopback || parsed.port().is_none() {
        return Err(CmdError::click(format!(
            "placement probe must use loopback HTTP with an explicit port: {url}"
        )));
    }
    let url_payload = STANDARD.encode(url.as_bytes());
    let attempts = attempts.max(1);
    let script = format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
url=$(printf '%s' '{url_payload}' | /usr/bin/base64 "$decode")
attempt=0
while [ "$attempt" -lt '{attempts}' ]; do
  attempt=$((attempt + 1))
  status=$(/usr/bin/curl --silent --show-error --max-time 10 --output /dev/null --write-out '%{{http_code}}' "$url" 2>/dev/null || true)
  case "$status" in 2??) printf 'STADO_PLACEMENT_HEALTH\tok\t%s\n' "$status"; exit 0 ;; esac
  [ "$attempt" -ge '{attempts}' ] || /bin/sleep 1
done
printf 'health probe failed: %s returned HTTP %s\n' "$url" "$status" >&2
exit 69
"#
    );
    let output = run_host_script(target, &script, runner, "health probe").await?;
    if marker_line(&output, "STADO_PLACEMENT_HEALTH\tok\t").is_none() {
        return Err(CmdError::click(format!(
            "{}: health probe returned no marker for {url}",
            target.name
        )));
    }
    Ok(())
}

async fn apply_routes(
    context: &MoveContext,
    destination: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    for route in &context.profile.routing {
        let route_target = target(&context.registry, &route.host)?;
        let action = if route.active_when_destination == destination {
            UnitAction::Start
        } else {
            UnitAction::Retire
        };
        act_on_unit(route_target, &route.unit, action, runner).await?;
    }
    Ok(())
}

async fn preflight(context: &MoveContext, runner: &Runner) -> Result<(), CmdError> {
    let source_profile = profile_host(&context.profile, &context.source.name)?;
    let destination_profile = profile_host(&context.profile, &context.destination.name)?;

    for logical in &context.profile.services {
        let source_spec = unit(source_profile, logical)?;
        let source_status = probe_unit(&context.source, source_spec, runner).await?;
        if !source_status.present || !source_status.loaded {
            return Err(CmdError::click(format!(
                "{}: source unit {} must be installed and running before migration",
                context.source.name, source_spec.unit
            )));
        }
        let destination_spec = unit(destination_profile, logical)?;
        let destination_status = probe_unit(&context.destination, destination_spec, runner).await?;
        if !destination_status.present {
            return Err(CmdError::click(format!(
                "{}: destination unit file is missing: {}",
                context.destination.name, destination_spec.path
            )));
        }
        if destination_status.loaded {
            return Err(CmdError::click(format!(
                "{}: destination unit {} is already running; refusing two active copies",
                context.destination.name, destination_spec.unit
            )));
        }
    }
    for probe in &source_profile.probes {
        health_probe(&context.source, &probe.url, 1, runner).await?;
    }
    for state in &context.profile.state {
        let exists = state_exists(&context.source, state, runner).await?;
        if state.required && !exists {
            return Err(CmdError::click(format!(
                "{}: required state file is missing: $HOME/{}",
                context.source.name, state.path
            )));
        }
    }
    for route in &context.profile.routing {
        let route_target = target(&context.registry, &route.host)?;
        let status = probe_unit(route_target, &route.unit, runner).await?;
        if !status.present {
            return Err(CmdError::click(format!(
                "{}: routing unit file is missing: {}",
                route.host, route.unit.path
            )));
        }
    }
    Ok(())
}

async fn release_claim(transaction_id: &str) -> Result<(), CmdError> {
    let mut last_error = None;
    for _ in 0..3 {
        let (mut document, generation) = registry::fetch_versioned_document().await?;
        if !placement::release_transaction(&mut document, transaction_id)
            .map_err(CmdError::click)?
        {
            return Ok(());
        }
        match registry::push_document_if(&document, &generation).await {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| CmdError::click("could not release placement transaction")))
}

fn destination_record(
    destination: &ComputeTarget,
    spec: &PlacementUnit,
    managed_since: &str,
) -> ManagedService {
    let mut managed = if spec.kind == "launchd" {
        service::launchd_service(
            &destination.name,
            &spec.unit,
            &spec.path,
            SOURCE_REGISTRY,
            managed_since,
        )
    } else {
        service::systemd_service(
            &destination.name,
            &spec.unit,
            &spec.path,
            SOURCE_REGISTRY,
            managed_since,
        )
    };
    managed.name = spec.name.clone();
    managed.host_heuristic = destination.host_heuristic.clone();
    managed
}

fn prepare_committed_document(context: &MoveContext) -> Result<Value, CmdError> {
    let source_profile = profile_host(&context.profile, &context.source.name)?;
    let destination_profile = profile_host(&context.profile, &context.destination.name)?;
    let mut document = context.claimed_document.clone();
    let managed_since = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    for logical in &context.profile.services {
        let source_spec = unit(source_profile, logical)?;
        service::remove_service(&mut document, &context.source.name, &source_spec.unit)
            .map_err(deploy_error)?;
        let destination_spec = unit(destination_profile, logical)?;
        let managed = destination_record(&context.destination, destination_spec, &managed_since);
        service::add_service(&mut document, &managed).map_err(deploy_error)?;
    }
    crate::service_resolution::retarget_profile(
        &mut document,
        &context.profile.name,
        &context.destination.name,
    )
    .map_err(CmdError::click)?;
    if !placement::release_transaction(&mut document, &context.transaction.id)
        .map_err(CmdError::click)?
    {
        return Err(CmdError::click(
            "placement transaction disappeared before commit",
        ));
    }
    Ok(document)
}

async fn rollback(context: &MoveContext, progress: &Progress, runner: &Runner) -> Vec<String> {
    let mut errors = Vec::new();
    let destination_profile = match profile_host(&context.profile, &context.destination.name) {
        Ok(profile) => profile,
        Err(error) => {
            errors.push(error.to_string());
            return errors;
        }
    };
    let source_profile = match profile_host(&context.profile, &context.source.name) {
        Ok(profile) => profile,
        Err(error) => {
            errors.push(error.to_string());
            return errors;
        }
    };

    if progress.destination_started {
        for logical in &context.profile.stop_order {
            match unit(destination_profile, logical) {
                Ok(spec) => {
                    if let Err(error) =
                        act_on_unit(&context.destination, spec, UnitAction::Retire, runner).await
                    {
                        errors.push(error.to_string());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    for path in progress.destination_written.iter().rev() {
        if let Err(error) =
            restore_state(&context.destination, path, &context.transaction.id, runner).await
        {
            errors.push(error.to_string());
        }
    }
    if progress.route_applied {
        if let Err(error) = apply_routes(context, &context.source.name, runner).await {
            errors.push(error.to_string());
        }
    }
    if progress.source_stopped || progress.source_retired {
        for logical in &context.profile.start_order {
            match unit(source_profile, logical) {
                Ok(spec) => {
                    if let Err(error) =
                        act_on_unit(&context.source, spec, UnitAction::Start, runner).await
                    {
                        errors.push(error.to_string());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    errors
}

async fn execute_move(
    context: &MoveContext,
    progress: &mut Progress,
    runner: &Runner,
    committer: &RegistryCommitter,
) -> Result<String, CmdError> {
    preflight(context, runner).await?;
    let source_profile = profile_host(&context.profile, &context.source.name)?;
    let destination_profile = profile_host(&context.profile, &context.destination.name)?;

    println!(
        "moving {}: {} -> {}",
        context.profile.name, context.source.name, context.destination.name
    );
    progress.source_stopped = true;
    for logical in &context.profile.stop_order {
        let spec = unit(source_profile, logical)?;
        act_on_unit(&context.source, spec, UnitAction::Stop, runner).await?;
        println!("  fenced {}:{}", context.source.name, logical);
    }

    let mut snapshots = Vec::with_capacity(context.profile.state.len());
    for state in &context.profile.state {
        snapshots.push(read_state(&context.source, state, runner).await?);
    }
    for snapshot in &snapshots {
        progress
            .destination_written
            .push(snapshot.spec.path.clone());
        write_state(
            &context.destination,
            snapshot,
            &context.transaction.id,
            runner,
        )
        .await?;
        println!("  transferred $HOME/{}", snapshot.spec.path);
    }

    progress.route_applied = true;
    apply_routes(context, &context.destination.name, runner).await?;
    progress.destination_started = true;
    for logical in &context.profile.start_order {
        let spec = unit(destination_profile, logical)?;
        act_on_unit(&context.destination, spec, UnitAction::Start, runner).await?;
        println!("  started {}:{}", context.destination.name, logical);
    }
    for probe in &destination_profile.probes {
        health_probe(&context.destination, &probe.url, 30, runner).await?;
        println!("  healthy {}:{}", context.destination.name, probe.service);
    }

    for logical in &context.profile.stop_order {
        let spec = unit(source_profile, logical)?;
        act_on_unit(&context.source, spec, UnitAction::Retire, runner).await?;
    }
    progress.source_retired = true;

    let committed = prepare_committed_document(context)?;
    committer(committed, context.claim_generation.clone()).await
}

async fn delegate_to_registry_authority(
    document: &Value,
    registry: &Registry,
    requested: &[String],
    to_host: &str,
    json_output: bool,
) -> Result<bool, CmdError> {
    let Some(directory) =
        crate::service_resolution::directory(document).map_err(CmdError::click)?
    else {
        return Ok(false);
    };
    let hostname = crate::providers::vast::system_hostname();
    let local = registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "placement host {hostname:?} has no registry target identity"
            ))
        })?;
    if local.name == directory.authority.target {
        return Ok(false);
    }
    let authority = registry
        .lookup(&directory.authority.target)
        .ok_or_else(|| CmdError::click("registry authority target disappeared"))?;
    let ssh = authority
        .ssh
        .as_deref()
        .ok_or_else(|| CmdError::click("registry authority has no SSH transport"))?;
    let mut argv = vec![
        directory.authority.command,
        "placement".to_string(),
        "move".to_string(),
        "--to-host".to_string(),
        to_host.to_string(),
    ];
    if json_output {
        argv.push("--json".to_string());
    }
    argv.extend(requested.iter().cloned());
    let remote_command = argv
        .iter()
        .map(|argument| crate::deploy::shlex_quote(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let status = tokio::process::Command::new("ssh")
        .args([
            "-T",
            "-F",
            "/dev/null",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=10",
            ssh,
        ])
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map_err(|error| CmdError::click(format!("registry authority SSH failed: {error}")))?;
    if !status.success() {
        return Err(CmdError::click(format!(
            "registry authority placement exited with {status}"
        )));
    }
    Ok(true)
}

async fn move_services(
    requested: &[String],
    to_host: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let parsed_registry = parse_registry(&document)?;
    if delegate_to_registry_authority(&document, &parsed_registry, requested, to_host, json_output)
        .await?
    {
        return Ok(());
    }
    let profile = placement::profile_for_services(&document, requested).map_err(CmdError::click)?;
    let _destination_profile = profile_host(&profile, to_host)?;
    let destination = target(&parsed_registry, to_host)?.clone();
    let sources = declared_profile_hosts(&parsed_registry, &profile)?;
    let source_name = match sources.as_slice() {
        [source] => source.clone(),
        [] => {
            return Err(CmdError::click(format!(
                "placement profile {:?} has no complete managed source",
                profile.name
            )))
        }
        _ => {
            return Err(CmdError::click(format!(
                "placement profile {:?} is active on multiple hosts: {}",
                profile.name,
                sources.join(", ")
            )))
        }
    };
    if source_name == to_host {
        let report = json!({
            "status": "already_placed",
            "profile": profile.name,
            "host": to_host,
        });
        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!("{} is already placed on {}", profile.name, to_host);
        }
        return Ok(());
    }
    let source = target(&parsed_registry, &source_name)?.clone();
    let transaction = PlacementTransaction {
        id: uuid::Uuid::new_v4().to_string(),
        profile: profile.name.clone(),
        from_host: source.name.clone(),
        to_host: destination.name.clone(),
        started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    let mut claimed_document = document;
    placement::claim_transaction(&mut claimed_document, &transaction).map_err(CmdError::click)?;
    let claim_generation = registry::push_document_if(&claimed_document, &generation).await?;
    let context = MoveContext {
        profile,
        source,
        destination,
        registry: parsed_registry,
        claimed_document,
        claim_generation,
        transaction,
    };
    let runner = production_runner();
    let committer = production_committer();
    let mut progress = Progress::default();
    match execute_move(&context, &mut progress, &runner, &committer).await {
        Ok(committed_generation) => {
            for path in &progress.destination_written {
                if let Err(error) = cleanup_state_backup(
                    &context.destination,
                    path,
                    &context.transaction.id,
                    &runner,
                )
                .await
                {
                    eprintln!("Warning: {error}");
                }
            }
            let report = json!({
                "status": "moved",
                "transaction_id": context.transaction.id,
                "profile": context.profile.name,
                "from_host": context.source.name,
                "to_host": context.destination.name,
                "registry_generation": committed_generation,
            });
            if json_output {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "moved {} from {} to {} (registry generation {})",
                    context.profile.name,
                    context.source.name,
                    context.destination.name,
                    committed_generation
                );
            }
            Ok(())
        }
        Err(primary) => {
            let rollback_errors = rollback(&context, &progress, &runner).await;
            let release_error = release_claim(&context.transaction.id).await.err();
            let mut details = vec![primary.to_string()];
            if !rollback_errors.is_empty() {
                details.push(format!("rollback failures: {}", rollback_errors.join("; ")));
            }
            if let Some(error) = release_error {
                details.push(format!("registry lock release failed: {error}"));
            }
            Err(CmdError::click(details.join("; ")))
        }
    }
}

// ---------------------------------------------------------------------------
// host publish-placement-policy
// ---------------------------------------------------------------------------

/// Basename the policy takes in the target's delivered-files directory, and the
/// only name [`APPLY_SCRIPT`] will read.
const POLICY_FILE: &str = "placement-policy.json";

/// Where the worker reads it, per `weles/src/worker/placement-policy.ts`: the
/// loader joins `homedir()` with `.config/weles/placement-policy.json` unless
/// `WELES_PLACEMENT_POLICY_FILE` overrides it. Reported here, never written
/// here — the move belongs to the remote script, because only the host can see
/// what the document replaced.
const POLICY_DESTINATION: &str = "$HOME/.config/weles/placement-policy.json";

/// The fixed remote script that moves a delivered policy into place and reports
/// both sides of the change, embedded in this binary. It takes no arguments and
/// both of its paths are fixed, so the only thing an operator can vary is what
/// the registry says. Kept as a checked-in file rather than a string literal so
/// it is reviewed and read as the shell program it is.
const APPLY_SCRIPT: &str = r#"#!/bin/sh
# Move the registry-published Weles placement policy into the path the worker
# reads, or refuse and change nothing.
#
# This script is embedded in the stado binary itself
# (`placement::APPLY_SCRIPT`, via include_str!). `stado host
# publish-placement-policy` delivers the document to
# $HOME/.stado/files/placement-policy.json through the audited channel and then
# runs this as one fixed remote script. It takes no arguments on purpose: a
# script that accepted a source or a destination path would be a remote writer
# with the audit trail removed. Both paths below are fixed, so the only thing
# an operator can vary is what the registry says.
#
# Three refusals, all of them silent failures somewhere else:
#
#   not JSON      a truncated or half-written delivery. Installing it takes the
#                 worker's placement loader out entirely, on every claim.
#   no _source    an unstamped document is one nobody can trace to a registry
#                 read. That is the file this whole change exists to retire: the
#                 host copy that disagreed with the registry for hours and could
#                 not be dated, attributed, or compared against it.
#   not this host a policy whose entries name no identity of this machine does
#                 not fail loudly in the worker. It resolves to `enabled: false`
#                 and the worker declines every row in silence -- 29,616 times,
#                 the last time this fleet learned it.
#
# The destination is written through a temporary file in the same directory and
# renamed, so a worker reading concurrently sees either the whole old document
# or the whole new one, never a partial write.
set -eu

src="$HOME/.stado/files/placement-policy.json"
dest_dir="$HOME/.config/weles"
dest="$dest_dir/placement-policy.json"

# Every check below is a JSON question, so a host without jq cannot answer any
# of them -- and a script that cannot verify must not write. The fleet's other
# scripts hardcode /usr/bin/jq; that path is real on the Linux hosts and absent
# on the macOS ones, where Homebrew owns it, so this one looks in the three
# places it is actually installed rather than assuming one of them.
jq=
for candidate in /usr/bin/jq /opt/homebrew/bin/jq /usr/local/bin/jq; do
  if [ -x "$candidate" ]; then
    jq=$candidate
    break
  fi
done
if [ -z "$jq" ]; then
  printf '%s\n' 'no jq on this host: refusing to install a placement policy nothing here can parse' >&2
  exit 69
fi

refuse() {
  printf 'refusing to install %s: %s\n' "$src" "$1" >&2
  exit 1
}

# The worker's own identity rule, transcribed from weles
# src/worker/identity.ts: trim, lowercase, drop trailing dots. `hostname` and
# node's `os.hostname()` are both gethostname(2), so this compares the same
# string the loader will compare.
host=$(hostname 2>/dev/null || printf '%s' '')
if [ -z "$host" ]; then
  printf '%s\n' 'this host cannot state its own hostname; the worker resolves placement by it' >&2
  exit 69
fi

# `norm` and `entry` are the loader's matching rule, applied to whichever
# document is being read: the delivery on the way in, and the file already in
# place on the way out.
filter='def norm: ascii_downcase | sub("\\.+$"; "");
def entry($h): [ .hosts[]? | select(((.hostname // "") | tostring | norm) == $h
    or (((.aliases // []) | map(tostring | norm)) | index($h) != null)) ] | .[0];'

# generation, enabled, actions -- as three tab-separated fields, for whichever
# entry belongs to this machine. `stado host publish-placement-policy` parses
# these to report the delta; an operator reading the remote output sees them
# directly.
summarize() {
  "$jq" -r --arg host "$host" "$filter"'
    ($host | norm) as $h
    | entry($h) as $e
    | [ ((._source.registry_generation // "unstamped") | tostring),
        (if $e == null then "-" else ($e.enabled | tostring) end),
        (if $e == null then "-"
         elif (($e.actions // []) | length) == 0 then "-"
         else (($e.actions | map(tostring)) | join(",")) end) ]
    | @tsv' "$1"
}

[ -f "$src" ] || refuse 'no delivered document at that path'
"$jq" -e 'type == "object"' "$src" >/dev/null 2>&1 || refuse 'it does not parse as a JSON object'
"$jq" -e '(._source | type) == "object"
  and ((._source.registry_generation // "") | tostring | length) > 0
  and ((._source.published_at // "") | tostring | length) > 0
  and ((._source.by // "") | tostring | length) > 0' "$src" >/dev/null 2>&1 \
  || refuse 'it carries no _source stamp naming the registry generation it came from'
"$jq" -e '.schema_version == 1 and (.hosts | type) == "array"' "$src" >/dev/null 2>&1 \
  || refuse 'the worker parses schema_version 1 with a hosts array, and this is not that'
"$jq" -e --arg host "$host" "$filter"'($host | norm) as $h | entry($h) != null' "$src" \
  >/dev/null 2>&1 \
  || refuse "no entry names this host ($host), so the worker would silently refuse every action"

# Read what is already there BEFORE overwriting it: after the rename nothing on
# this machine can still say what the host was running on.
if [ -L "$dest" ]; then
  printf 'refusing to write through a symlink: %s\n' "$dest" >&2
  exit 1
elif [ ! -e "$dest" ]; then
  previous=$(printf 'absent\t-\t-')
else
  previous=$(summarize "$dest" 2>/dev/null || printf '%s' '')
  [ -n "$previous" ] || previous=$(printf 'unreadable\t-\t-')
fi

/bin/mkdir -p "$dest_dir"
tmp="$dest_dir/.placement-policy.json.stado-apply.$$"
trap '/bin/rm -f "$tmp"' EXIT
/bin/cp "$src" "$tmp"
/bin/chmod 600 "$tmp"
/bin/mv "$tmp" "$dest"
trap - EXIT

installed=$(summarize "$dest")
generation=$(printf '%s' "$installed" | /usr/bin/cut -f1)

printf 'PLACEMENT_VANTAGE\t%s\n' "$host"
printf 'PLACEMENT_POLICY\tprevious\t%s\n' "$previous"
printf 'PLACEMENT_POLICY\tinstalled\t%s\n' "$installed"
printf 'installed %s at registry generation %s\n' "$dest" "$generation"
"#;

/// `PLACEMENT_POLICY <phase> <generation> <enabled> <actions>`, tab separated:
/// the apply script's report of what the host carried and what it carries now.
const POLICY_MARKER: &str = "PLACEMENT_POLICY";

/// `PLACEMENT_VANTAGE <hostname>`: the name the host gave for itself, which is
/// the string the worker's loader will match its entry against. Printed because
/// a policy that names every host except the one it is installed on is not an
/// error the worker reports — it is a worker that declines everything.
const VANTAGE_MARKER: &str = "PLACEMENT_VANTAGE";

/// What `_source.by` names, so a file on a host traces back to the command that
/// wrote it rather than to a machine that happened to have write access.
const PUBLISHED_BY: &str = "stado host publish-placement-policy";

/// The one document shape the worker's loader parses (`schema_version must be
/// 1`, `placement-policy.ts`). Publishing anything else delivers a file the
/// consumer refuses.
const POLICY_SCHEMA_VERSION: u64 = 1;

/// How long a worker keeps a successfully loaded policy before reading the file
/// again — `CACHE_TTL_MS = 30_000` in `placement-policy.ts`. Reported so an
/// operator knows whether a still-refusing worker is stale or wrong.
const POLICY_CACHE_SECONDS: u64 = 30;

/// The worker's own hostname rule, transcribed from `normalizeHostname` in
/// `weles/src/worker/identity.ts`: trim, lowercase, drop trailing dots.
///
/// Transcribed rather than approximated because it is a comparison, and a
/// comparison the two sides perform differently is a host that matches nothing.
fn normalize_hostname(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .trim_end_matches('.')
        .to_string()
}

/// Every name a target answers to: the registry name first, its declared
/// `hostnames` as aliases.
///
/// The registry name leads because that is what an operator types and what the
/// rest of this binary keys on. The declared hostnames have to be there because
/// the worker matches `os.hostname()`, which on a Mac is the `.local` form —
/// a document carrying only the registry name resolves to no entry, and no
/// entry is a worker that silently refuses every action rather than an error.
///
/// Deduplicated: the loader rejects an entry that declares one identity twice,
/// and a registry that lists a target's own name under `hostnames` is common.
fn identities(target: &ComputeTarget) -> Result<(String, Vec<String>), CmdError> {
    let hostname = normalize_hostname(&target.name);
    if hostname.is_empty() {
        return Err(CmdError::click(
            "the target has no name to publish a placement policy under",
        ));
    }
    let mut aliases: Vec<String> = Vec::new();
    for declared in &target.hostnames {
        let alias = normalize_hostname(declared);
        if alias.is_empty() || alias == hostname || aliases.contains(&alias) {
            continue;
        }
        aliases.push(alias);
    }
    Ok((hostname, aliases))
}

/// The registry's action list, checked against the grammar the consumer
/// enforces (`ACTION_RE` and `parseActions`, `placement-policy.ts`).
///
/// Checked before delivery, because the loader THROWS on a list it dislikes and
/// a worker whose placement load throws claims nothing at all. A single typo in
/// the registry would otherwise become a stopped worker discovered by absence —
/// the same shape of failure this command was written to end, arriving by the
/// same route.
fn checked_actions(target: &str, weles: &WelesPolicy) -> Result<Vec<String>, CmdError> {
    let mut seen = BTreeSet::new();
    for action in &weles.actions {
        let legible = !action.is_empty()
            && action.trim() == action
            && (action == "*"
                || action.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                }));
        if !legible {
            return Err(CmdError::click(format!(
                "{target} declares the weles action {action:?}, which the worker's placement \
                 loader refuses: an action is '*', or lowercase letters, digits and \
                 underscores. A list the loader refuses is not a narrower policy — it is a \
                 worker that claims nothing"
            )));
        }
        if !seen.insert(action.as_str()) {
            return Err(CmdError::click(format!(
                "{target} declares the weles action {action:?} twice, and the worker's loader \
                 refuses a list with duplicates"
            )));
        }
    }
    if seen.contains("*") && seen.len() != usize::from(true) {
        return Err(CmdError::click(format!(
            "{target} declares the weles wildcard alongside named actions; the loader requires \
             '*' to stand alone, because a list that says both does not say which one wins"
        )));
    }
    if weles.enabled && weles.actions.is_empty() {
        return Err(CmdError::click(format!(
            "{target} declares weles.enabled with an empty action list. The worker resolves \
             that to disabled — its loader computes `enabled && actions.length > 0` — so \
             publishing it would deliver a document that says one thing and does the other. \
             Settle it in the registry first"
        )));
    }
    Ok(weles.actions.clone())
}

/// One side of the change, as the host reported it.
struct PolicySnapshot {
    /// Registry generation the document was stamped with, or the script's word
    /// for a file that carried no stamp, could not be parsed, or was not there.
    generation: String,
    /// `true`, `false`, or `-` when no entry on that host named this machine.
    enabled: String,
    actions: Vec<String>,
}

/// Read one `PLACEMENT_POLICY <phase> ...` line out of the script's output.
///
/// The before-state arrives as remote output rather than as anything this
/// process knows, because the file it replaced only ever existed on that host.
/// A missing line is reported as missing and never defaulted to "the same as
/// now": defaulting would render every publication as a no-op and hide exactly
/// the drift this command exists to close.
fn snapshot(stdout: &str, phase: &str) -> Option<PolicySnapshot> {
    let prefix = format!("{POLICY_MARKER}\t{phase}\t");
    let line = stdout.lines().find(|line| line.starts_with(&prefix))?;
    let mut fields = line[prefix.len()..].split('\t');
    Some(PolicySnapshot {
        generation: fields.next()?.to_string(),
        enabled: fields.next()?.to_string(),
        actions: fields.next().map(action_list).unwrap_or_default(),
    })
}

/// `-` is the script's word for "no entry, or an empty list", not an action
/// named `-`.
fn action_list(field: &str) -> Vec<String> {
    if field == "-" {
        return Vec::new();
    }
    field
        .split(',')
        .filter(|action| !action.is_empty())
        .map(str::to_string)
        .collect()
}

/// An empty difference has to read as empty, not as a blank line an operator
/// scanning a delta will fill in with an assumption.
fn or_none(actions: &[&str]) -> String {
    if actions.is_empty() {
        "(none)".to_string()
    } else {
        actions.join(", ")
    }
}

/// `stado host publish-placement-policy TARGET [--json]` — put the registry's
/// `weles` declaration onto the host, stamped with the generation it came from.
///
/// The registry declares `weles.actions` per target and the worker never reads
/// it. The worker reads `~/.config/weles/placement-policy.json` on the box it
/// runs on. Those two disagreed: the registry listed `apple_create_developer_id`
/// and the host file did not, so the worker skipped the row in silence for hours
/// while the registry said it was allowed. Two sources of truth, and the one an
/// operator edits was not the one that decided.
///
/// This makes the host file a cache. It is generated from the registry, stamped
/// with `_source`, delivered over the audited channel, and refused on arrival if
/// the stamp is missing. Nothing here lets an operator put a list on a host that
/// the registry does not already declare — the only input is a target name.
///
/// It reports the delta rather than a success word. "Published" tells an
/// operator nothing; the generation it replaced and the actions that came and
/// went are the whole content of the operation, and an unchanged list is itself
/// an answer worth reading.
#[allow(clippy::too_many_lines)]
pub async fn publish_placement_policy(
    target_name: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    let declared = parse_registry(&document)?;
    let resolved = target(&declared, target_name)?.clone();
    let weles = resolved.weles.as_ref().ok_or_else(|| {
        CmdError::click(format!(
            "{} declares no `weles` block in the registry, so there is nothing to publish. \
             Declare weles.enabled and weles.actions there first: a policy invented here \
             would be the second source of truth this command exists to remove",
            resolved.name
        ))
    })?;
    let actions = checked_actions(&resolved.name, weles)?;
    let (hostname, aliases) = identities(&resolved)?;

    let published_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let policy = json!({
        "_source": {
            "registry_generation": generation,
            "published_at": published_at,
            "by": PUBLISHED_BY,
        },
        "schema_version": POLICY_SCHEMA_VERSION,
        "hosts": [{
            "hostname": hostname,
            "aliases": aliases,
            "enabled": weles.enabled,
            "actions": actions,
        }],
    });

    // Staged as a file because the delivery channel carries files: the same
    // delivered-file path any other artifact takes, checksummed on arrival,
    // rather than a private scp with the audit trail removed.
    let staged = tempfile::Builder::new()
        .prefix("stado-placement-policy-")
        .suffix(".json")
        .tempfile()?;
    std::fs::write(
        staged.path(),
        format!("{}\n", serde_json::to_string_pretty(&policy)?),
    )?;
    let source = staged
        .path()
        .to_str()
        .ok_or_else(|| CmdError::click("the staged policy path is not valid UTF-8"))?;
    let (delivered, bytes) = super::host::deliver_file(&resolved.name, source, POLICY_FILE).await?;

    let runner = production_runner();
    let reported = host_channel::run_fixed_script(&resolved.name, APPLY_SCRIPT, &runner)
        .await
        .map_err(|error| {
            // Delivered and not installed is a real state, and the operator has
            // to be told which half happened: the worker is still running the
            // old list, and a file it does not read is sitting next to it. The
            // refusal is the remote's own words -- the script says exactly
            // which of its checks the document failed.
            CmdError::click(format!(
                "{name}: the policy reached {delivered} and was NOT installed: {error}. \
                 Settle the refusal and publish again",
                name = resolved.name
            ))
        })?;

    let installed = snapshot(&reported, "installed").ok_or_else(|| {
        CmdError::click(format!(
            "{}: the apply script reported no installed policy, so {POLICY_DESTINATION} on that \
             host is now of unknown provenance; read it there before publishing again",
            resolved.name
        ))
    })?;
    let previous = snapshot(&reported, "previous");
    let vantage_prefix = format!("{VANTAGE_MARKER}\t");
    let vantage = reported
        .lines()
        .find_map(|line| line.strip_prefix(&vantage_prefix))
        .unwrap_or("-")
        .trim();

    // The delta is computed from what the host reports it now carries, not from
    // what was sent: the two are the same only if the script installed exactly
    // the document that was delivered, and that is the claim worth checking.
    let before: BTreeSet<&str> = previous
        .iter()
        .flat_map(|held| held.actions.iter().map(String::as_str))
        .collect();
    let after: BTreeSet<&str> = installed.actions.iter().map(String::as_str).collect();
    let added: Vec<&str> = after.difference(&before).copied().collect();
    let removed: Vec<&str> = before.difference(&after).copied().collect();
    let unchanged: Vec<&str> = after.intersection(&before).copied().collect();
    let current: Vec<&str> = installed.actions.iter().map(String::as_str).collect();
    let previous_generation = previous
        .as_ref()
        .map_or("unreported", |held| held.generation.as_str());
    let previous_enabled = previous
        .as_ref()
        .map_or("unreported", |held| held.enabled.as_str());

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "vantage": vantage,
                "delivered": delivered,
                "bytes": bytes,
                "installed": POLICY_DESTINATION,
                "registry_generation": generation,
                "previous_generation": previous_generation,
                "published_at": published_at,
                "enabled": installed.enabled,
                "previous_enabled": previous_enabled,
                "actions": current,
                "previous_actions": previous.as_ref().map(|held| held.actions.clone()),
                "added": added,
                "removed": removed,
                "unchanged": unchanged,
                "status": "published",
            }))?
        );
        return Ok(());
    }

    println!("target:      {}", resolved.name);
    println!("vantage:     {vantage}");
    println!("delivered:   {delivered} ({bytes} bytes)");
    println!("installed:   {POLICY_DESTINATION}");
    println!(
        "generation:  {previous_generation} -> {}",
        installed.generation
    );
    println!("enabled:     {previous_enabled} -> {}", installed.enabled);
    println!("actions:     {}", or_none(&current));
    println!("added:       {}", or_none(&added));
    println!("removed:     {}", or_none(&removed));
    println!("unchanged:   {}", or_none(&unchanged));

    // What the replaced file was is worth a sentence of its own. None of these
    // three is trivia: an unstamped file is the pre-provenance cache this
    // command retires, an unparseable one is a worker that had been failing its
    // placement load on every claim, and an absent one is a worker that had
    // nothing to load at all. All three were silent.
    match previous_generation {
        "unstamped" => println!(
            "\nthe file it replaced carried no _source: nothing on that host could say which \
             registry read produced it, or when"
        ),
        "unreadable" => println!(
            "\nthe file it replaced did not parse: the worker's loader had been throwing on \
             every read, and a worker that cannot load placement claims nothing"
        ),
        "absent" => println!(
            "\nthere was no policy file on that host: under WELES_PLACEMENT_MODE=required the \
             worker had been refusing every action for want of one"
        ),
        _ => {}
    }

    if added.is_empty() && removed.is_empty() {
        println!(
            "\nno action changed: {} was already carrying this list, and now carries the \
             registry generation that proves where it came from",
            resolved.name
        );
    } else {
        println!(
            "\n{} may now run what the registry declares and nothing else; its worker re-reads \
             {POLICY_DESTINATION} within {POLICY_CACHE_SECONDS} seconds",
            resolved.name
        );
    }
    Ok(())
}
