//! `stado placement move` — one fenced transaction for a colocated service group.
//!
//! The registry profile is the complete operational contract: concrete units
//! per host, stop/start order, durable files, loopback health probes, and routing
//! units. The command claims the profile through registry CAS, fences the source,
//! copies state only after writers stop, activates and probes the destination,
//! then commits the service declarations with a second CAS. Every failure before
//! that commit restores destination files, routing, and source services.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{SecondsFormat, Utc};
use clap::Subcommand;
use serde_json::{json, Value};

use crate::deploy::service::{self, ManagedService, SOURCE_REGISTRY};
use crate::deploy::{host_channel, production_runner, CommandOutput, DeployError, Runner};
use crate::placement::{
    self, PlacementHost, PlacementProfile, PlacementState, PlacementTransaction, PlacementUnit,
};
use crate::targets::{self, ComputeTarget, Registry};

use super::{registry, CmdError};

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
}

pub async fn dispatch(command: PlacementCommands) -> Result<(), CmdError> {
    match command {
        PlacementCommands::Move {
            services,
            to_host,
            json,
        } => move_services(&services, &to_host, json).await,
    }
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
    registry::push_document_if(&committed, &context.claim_generation).await
}

async fn move_services(
    requested: &[String],
    to_host: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let profile = placement::profile_for_services(&document, requested).map_err(CmdError::click)?;
    let parsed_registry = parse_registry(&document)?;
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
    let mut progress = Progress::default();
    match execute_move(&context, &mut progress, &runner).await {
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
