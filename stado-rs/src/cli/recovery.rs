//! `stado recovery migrate` — fenced, provider-neutral storage cutover.
//!
//! The command deliberately keeps the queue paused at every failure boundary.
//! It opens an optional GCP billing window only around source reads, closes it
//! before any workload is resumed, and switches only explicitly named services
//! and compute providers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::num::NonZeroUsize;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use crate::deploy::{host_channel, production_runner, service, DeployError};
use crate::queue::control;
use crate::queue::copy::{CopyOptions, Endpoint, DEFAULT_CONCURRENCY};
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

use super::storage::EndpointArgs;
use super::{storage, CmdError};

const BILLING_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const CLOUD_BILLING_BASE: &str = "https://cloudbilling.googleapis.com/v1/projects";
const FENCE_REASON: &str = "fenced by stado recovery migrate";
const ROUTING_OVERRIDES: &[&str] = &[
    "WC_STORAGE_BACKEND",
    "WC_BUCKET",
    "WC_AZURE_STORAGE_ACCOUNT",
    "WC_AZURE_CONTAINER",
    "WC_S3_BUCKET",
    "WC_S3_REGION",
    "WC_LOCAL_STORAGE_PATH",
    "WC_PROVIDERS",
    "WC_DISABLED_PROVIDERS",
];

#[derive(Subcommand, Debug)]
pub enum RecoveryCommands {
    /// Drain, copy, verify, cut over selected services, and optionally resume.
    Migrate(Box<RecoveryMigrateArgs>),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ServiceRef {
    host: String,
    service: String,
}

impl std::fmt::Display for ServiceRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.host, self.service)
    }
}

impl FromStr for ServiceRef {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (host, service) = raw.split_once(':').ok_or_else(|| {
            format!("{raw:?} must be HOST:SERVICE (for example mac-mini:stado-agent)")
        })?;
        if host.is_empty()
            || service.is_empty()
            || host.chars().any(char::is_whitespace)
            || service.chars().any(char::is_whitespace)
        {
            return Err(format!("{raw:?} must contain a non-empty HOST and SERVICE"));
        }
        Ok(Self {
            host: host.to_string(),
            service: service.to_string(),
        })
    }
}

#[derive(Args, Debug)]
pub struct RecoveryMigrateArgs {
    #[command(flatten)]
    ends: EndpointArgs,
    /// Every source writer Stado must stop before copying. HOST:SERVICE; repeatable. Omit only with --source-offline.
    #[arg(long = "writer")]
    writers: Vec<ServiceRef>,
    /// Assert that no unlisted source writer can run, including schedulers, Cloud Functions, Cloud Run jobs, coordinators, monitors, and agents.
    #[arg(long)]
    source_offline: bool,
    /// Service to restart on the destination after config cutover. HOST:SERVICE; repeatable. Every activated service is fenced first.
    #[arg(long = "activate")]
    activate: Vec<ServiceRef>,
    /// Complete compute-provider allowlist after cutover. Repeatable; gcp is rejected.
    #[arg(long = "enable-provider", required = true)]
    enable_providers: Vec<String>,
    /// Resume dispatch and claims after every other step. Without it the destination stays paused.
    #[arg(long)]
    resume: bool,
    /// Maximum seconds to wait for running/ to drain on each store.
    #[arg(long, default_value_t = control::default_drain_timeout_s())]
    drain_timeout: u64,
    /// Objects copied in parallel.
    #[arg(long, default_value_t = default_concurrency())]
    concurrency: NonZeroUsize,
    /// Config file to atomically rewrite. Defaults to Stado's resolved file.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Attach GCP billing only for source fencing, copy, and verification, then detach it.
    #[arg(long)]
    manage_gcp_billing: bool,
    /// GCP project whose billing window may be managed.
    #[arg(long)]
    gcp_project: Option<String>,
    /// Full billingAccounts/... name restored for the migration window.
    #[arg(long)]
    gcp_billing_account: Option<String>,
    /// Must exactly repeat --gcp-project before a billable API call is made.
    #[arg(long)]
    confirm_billing_window: Option<String>,
    /// Validate and print the plan; perform no network or filesystem writes and no billing change.
    #[arg(long)]
    dry_run: bool,
}

fn default_concurrency() -> NonZeroUsize {
    NonZeroUsize::new(DEFAULT_CONCURRENCY).expect("copy concurrency is non-zero")
}

struct PreparedConfig {
    path: PathBuf,
    bytes: Vec<u8>,
}

#[derive(Clone)]
struct ResolvedService {
    reference: ServiceRef,
    target: ComputeTarget,
    service: service::ManagedService,
    config_path: String,
    activate: bool,
}

pub async fn dispatch(command: RecoveryCommands) -> Result<(), CmdError> {
    match command {
        RecoveryCommands::Migrate(args) => migrate(&args).await,
    }
}

async fn migrate(args: &RecoveryMigrateArgs) -> Result<(), CmdError> {
    let source = args.ends.source();
    let destination = args.ends.destination();
    validate_args(args, &source, &destination)?;
    let prepared = prepare_config(args, &destination)?;
    if args.dry_run {
        print_plan(args, &source, &destination, &prepared);
        return Ok(());
    }

    let services = resolve_services(args).await?;
    println!("[1/9] fencing destination {}", destination.describe());
    let destination_store = endpoint_store(&destination).await?;
    control::set_paused(&destination_store, true, FENCE_REASON, "").await?;
    drain_store(
        &destination_store,
        &destination.describe(),
        args.drain_timeout,
    )
    .await?;

    let mut billing_attempted = false;
    if args.manage_gcp_billing {
        let project = required(args.gcp_project.as_deref(), "--gcp-project")?;
        let account = required(args.gcp_billing_account.as_deref(), "--gcp-billing-account")?;
        println!("[2/9] opening bounded GCP billing window for {project}");
        ensure_billing_disabled(project).await?;
        billing_attempted = true;
        if let Err(open_error) = update_gcp_billing(project, account).await {
            let close = close_billing_window(project).await;
            return Err(combine_billing_error(open_error, close));
        }
    } else {
        println!("[2/9] using source without a managed billing window");
    }

    let transfer_result = transfer(args, &source, &destination, &services).await;
    let close_result = if billing_attempted {
        let project = required(args.gcp_project.as_deref(), "--gcp-project")?;
        println!("[6/9] closing GCP billing window before cutover");
        close_billing_window(project).await
    } else {
        Ok(())
    };
    match (transfer_result, close_result) {
        (Err(transfer), Err(close)) => return Err(CmdError::click(format!("{transfer}; CRITICAL: transfer failed and the GCP billing window could not be closed: {close}. Both stores remain PAUSED"))),
        (Err(transfer), Ok(())) => return Err(transfer),
        (Ok(()), Err(close)) => return Err(CmdError::click(format!("CRITICAL: verified transfer completed, but the GCP billing window could not be closed: {close}. Cutover was not started and both stores remain PAUSED"))),
        (Ok(()), Ok(())) => {}
    }

    println!(
        "[7/9] atomically switching Stado config to {}",
        destination.describe()
    );
    install_local_config(&prepared)?;
    install_remote_configs(&services, &prepared.bytes).await?;
    println!("[8/9] restarting only explicitly activated services");
    restart_activated(&services).await?;
    if args.resume {
        println!(
            "[9/9] resuming dispatch and claims on {}",
            destination.describe()
        );
        control::set_paused(&destination_store, false, "", "").await?;
    } else {
        println!("[9/9] destination remains PAUSED (no --resume)");
    }
    println!(
        "recovery migration complete: {} -> {}; GCP compute is absent from the provider allowlist",
        source.describe(),
        destination.describe()
    );
    Ok(())
}

async fn transfer(
    args: &RecoveryMigrateArgs,
    source: &Endpoint,
    destination: &Endpoint,
    services: &[ResolvedService],
) -> Result<(), CmdError> {
    println!("[3/9] fencing and draining source {}", source.describe());
    let source_store = endpoint_store(source).await?;
    control::set_paused(&source_store, true, FENCE_REASON, "").await?;
    drain_store(&source_store, &source.describe(), args.drain_timeout).await?;
    println!("[4/9] stopping every declared writer before the final copy");
    stop_services(services).await?;
    println!("[5/9] copying the complete canonical namespace");
    storage::copy_between(
        source.clone(),
        destination.clone(),
        CopyOptions {
            prefixes: Vec::new(),
            concurrency: args.concurrency.get(),
        },
        false,
        false,
    )
    .await?;
    println!("[5/9] verifying names, metadata, and body bytes read-only");
    storage::verify_between(source.clone(), destination.clone(), &[], false).await?;
    Ok(())
}

fn validate_args(
    args: &RecoveryMigrateArgs,
    source: &Endpoint,
    destination: &Endpoint,
) -> Result<(), CmdError> {
    if source.describe() == destination.describe() {
        return Err(CmdError::usage(format!(
            "source and destination are the same store ({})",
            source.describe()
        )));
    }
    validate_endpoint(source, "source")?;
    validate_endpoint(destination, "destination")?;
    if args.source_offline && !args.writers.is_empty() {
        return Err(CmdError::usage("--source-offline conflicts with --writer; either list every writer or assert that none can run"));
    }
    if !args.source_offline && args.writers.is_empty() {
        return Err(CmdError::usage("list every source writer with --writer HOST:SERVICE, or pass --source-offline after independently disabling every writer"));
    }
    let mut providers = BTreeSet::new();
    for provider in &args.enable_providers {
        if provider == "gcp" {
            return Err(CmdError::usage(
                "--enable-provider gcp is forbidden during recovery migration",
            ));
        }
        if crate::capabilities::configurable_variant(
            crate::capabilities::CapabilityKind::Compute,
            provider,
        )
        .is_none()
        {
            return Err(CmdError::usage(format!(
                "unknown compute provider {provider:?}"
            )));
        }
        if !providers.insert(provider) {
            return Err(CmdError::usage(format!(
                "--enable-provider {provider} was repeated"
            )));
        }
    }
    if args.resume && args.activate.is_empty() {
        return Err(CmdError::usage(
            "--resume requires at least one explicitly selected --activate HOST:SERVICE",
        ));
    }
    if args.manage_gcp_billing {
        if source.kind != "gcs" {
            return Err(CmdError::usage(
                "--manage-gcp-billing is valid only when --from gcs",
            ));
        }
        let project = required(args.gcp_project.as_deref(), "--gcp-project")?;
        validate_gcp_project(project)?;
        let account = required(args.gcp_billing_account.as_deref(), "--gcp-billing-account")?;
        validate_billing_account(account)?;
        let confirmation = required(
            args.confirm_billing_window.as_deref(),
            "--confirm-billing-window",
        )?;
        if confirmation != project {
            return Err(CmdError::usage(format!(
                "--confirm-billing-window must exactly equal {project:?}"
            )));
        }
    } else if args.gcp_project.is_some()
        || args.gcp_billing_account.is_some()
        || args.confirm_billing_window.is_some()
    {
        return Err(CmdError::usage(
            "GCP billing flags require --manage-gcp-billing",
        ));
    }
    Ok(())
}

fn validate_endpoint(endpoint: &Endpoint, label: &str) -> Result<(), CmdError> {
    let missing = match endpoint.kind.as_str() {
        "gcs" | "s3" if endpoint.bucket.is_empty() => Some("bucket"),
        "azure" if endpoint.account.is_empty() => Some("storage account"),
        "azure" if endpoint.container.is_empty() => Some("container"),
        "local" if endpoint.path.is_empty() => Some("path"),
        _ => None,
    };
    if let Some(locator) = missing {
        return Err(CmdError::usage(format!(
            "{label} {} endpoint needs a {locator}",
            endpoint.kind
        )));
    }
    Ok(())
}

fn validate_gcp_project(project: &str) -> Result<(), CmdError> {
    let valid = !project.is_empty()
        && project
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(CmdError::usage(format!(
            "invalid GCP project id {project:?}"
        )))
    }
}

fn validate_billing_account(account: &str) -> Result<(), CmdError> {
    let Some(id) = account.strip_prefix("billingAccounts/") else {
        return Err(CmdError::usage(
            "--gcp-billing-account must be a full billingAccounts/... name",
        ));
    };
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(CmdError::usage(format!(
            "invalid billing account name {account:?}"
        )));
    }
    Ok(())
}

fn required<'a>(value: Option<&'a str>, flag: &str) -> Result<&'a str, CmdError> {
    value.ok_or_else(|| CmdError::usage(format!("{flag} is required")))
}

async fn endpoint_store(endpoint: &Endpoint) -> Result<JobStorage, CmdError> {
    let backend = endpoint.build().await?;
    Ok(JobStorage::with_backend_and_bucket(
        backend,
        endpoint.kind.clone(),
        endpoint.bucket.clone(),
    ))
}

async fn drain_store(
    store: &JobStorage,
    description: &str,
    timeout_seconds: u64,
) -> Result<(), CmdError> {
    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_seconds);
    let poll = Duration::from_secs(crate::constants::POLL_INTERVAL_S);
    loop {
        let running = control::job_count(store, control::RUNNING_PREFIX).await?;
        if running == 0 {
            let queued = control::job_count(store, control::QUEUED_PREFIX).await?;
            println!("  {description} drained: running=0, queued={queued}, paused=true");
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err(CmdError::click(format!("{description} drain timed out after {}s with {running} running job(s); both stores remain PAUSED", started.elapsed().as_secs())));
        }
        println!("  waiting for {description}: {running} job(s) still running");
        tokio::time::sleep(poll).await;
    }
}

async fn resolve_services(args: &RecoveryMigrateArgs) -> Result<Vec<ResolvedService>, CmdError> {
    let mut requested: BTreeMap<ServiceRef, bool> = BTreeMap::new();
    for reference in &args.writers {
        requested.entry(reference.clone()).or_insert(false);
    }
    for reference in &args.activate {
        requested.insert(reference.clone(), true);
    }
    let runner = production_runner();
    let mut resolved = Vec::new();
    for (reference, activate) in requested {
        let target = host_channel::canonical_target(&reference.host)
            .await
            .map_err(deploy_error)?;
        let matches: Vec<service::ManagedService> = service::declared_services(&target)
            .into_iter()
            .filter(|candidate| candidate.matches(&reference.service))
            .collect();
        let managed = match matches.as_slice() {
            [managed] => managed.clone(),
            [] => {
                return Err(CmdError::click(format!(
                    "{} has no registry-managed service named {:?}",
                    reference.host, reference.service
                )))
            }
            _ => {
                return Err(CmdError::click(format!(
                    "{} resolves ambiguously on {}",
                    reference.service, reference.host
                )))
            }
        };
        let unit = service::fetch_unit_file(&target, &managed, &runner)
            .await
            .map_err(deploy_error)?;
        let environment = service::unit_environment(&unit).map_err(deploy_error)?;
        if !environment.environment_files.is_empty() {
            return Err(CmdError::click(format!("{} uses EnvironmentFile entries; Stado cannot prove they do not override storage routing", reference)));
        }
        for override_name in ROUTING_OVERRIDES {
            if environment
                .env
                .iter()
                .any(|(name, _)| name == override_name)
            {
                return Err(CmdError::click(format!("{} hard-codes {override_name}; remove the routing override so STADO_CONFIG is authoritative", reference)));
            }
        }
        let config_path = environment
            .env
            .iter()
            .find_map(|(name, value)| (name == "STADO_CONFIG").then(|| value.clone()))
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{} has no STADO_CONFIG in its unit; refusing an unverifiable cutover",
                    reference
                ))
            })?;
        if !Path::new(&config_path).is_absolute() {
            return Err(CmdError::click(format!(
                "{} has non-absolute STADO_CONFIG={config_path:?}",
                reference
            )));
        }
        resolved.push(ResolvedService {
            reference,
            target,
            service: managed,
            config_path,
            activate,
        });
    }
    Ok(resolved)
}

fn deploy_error(error: DeployError) -> CmdError {
    CmdError::click(error.to_string())
}

async fn stop_services(services: &[ResolvedService]) -> Result<(), CmdError> {
    let runner = production_runner();
    for resolved in services {
        let report = service::stop_service(&resolved.target, &resolved.service, &runner)
            .await
            .map_err(deploy_error)?;
        if !report.succeeded("stopped") {
            return Err(CmdError::click(format!(
                "could not fence {}: {}",
                resolved.reference,
                report.failure()
            )));
        }
        println!("  stopped {}", resolved.reference);
    }
    Ok(())
}

async fn restart_activated(services: &[ResolvedService]) -> Result<(), CmdError> {
    let runner = production_runner();
    for resolved in services.iter().filter(|service| service.activate) {
        let report = service::restart_service(&resolved.target, &resolved.service, &runner)
            .await
            .map_err(deploy_error)?;
        if !report.succeeded("restarted") {
            return Err(CmdError::click(format!(
                "could not activate {}: {}; destination remains PAUSED",
                resolved.reference,
                report.failure()
            )));
        }
        println!("  restarted {}", resolved.reference);
    }
    Ok(())
}

fn prepare_config(
    args: &RecoveryMigrateArgs,
    destination: &Endpoint,
) -> Result<PreparedConfig, CmdError> {
    let path = match &args.config {
        Some(path) => crate::config_file::expand_tilde(&path.to_string_lossy()),
        None => crate::config_file::find_config_file().ok_or_else(|| CmdError::click("no Stado config file exists; pass --config PATH so recovery can perform an explicit atomic cutover"))?,
    };
    let text = fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&text)?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("{} must contain a JSON object", path.display())))?;
    root.insert(
        "providers".to_string(),
        Value::Array(
            args.enable_providers
                .iter()
                .map(|provider| Value::String(provider.clone()))
                .collect(),
        ),
    );
    root.insert("providers_disabled".to_string(), Value::Array(Vec::new()));
    set_storage_destination(root, destination)?;
    let problems = crate::config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "cutover config is invalid: {}",
            problems.join("; ")
        )));
    }
    let mut bytes = serde_json::to_vec_pretty(&document)?;
    bytes.push(b'\n');
    Ok(PreparedConfig { path, bytes })
}

fn set_storage_destination(
    root: &mut Map<String, Value>,
    destination: &Endpoint,
) -> Result<(), CmdError> {
    let storage = root
        .entry("storage".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click("config storage must be an object"))?;
    storage.insert(
        "backend".to_string(),
        Value::String(destination.kind.clone()),
    );
    let (section, fields): (&str, Vec<(&str, &str)>) = match destination.kind.as_str() {
        "gcs" => ("gcs", vec![("bucket", &destination.bucket)]),
        "azure" => (
            "azure",
            vec![
                ("account", &destination.account),
                ("container", &destination.container),
            ],
        ),
        "s3" => (
            "s3",
            vec![
                ("bucket", &destination.bucket),
                ("region", &destination.region),
            ],
        ),
        "local" => ("local", vec![("path", &destination.path)]),
        other => {
            return Err(CmdError::usage(format!(
                "unsupported destination {other:?}"
            )))
        }
    };
    let locator = storage
        .entry(section.to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("config storage.{section} must be an object")))?;
    for (name, value) in fields {
        locator.insert(name.to_string(), Value::String(value.to_string()));
    }
    if storage
        .get("backup")
        .and_then(Value::as_object)
        .and_then(|backup| backup.get("backend"))
        .and_then(Value::as_str)
        == Some("gcs")
        && destination.kind != "gcs"
    {
        storage.remove("backup");
    }
    Ok(())
}

fn install_local_config(prepared: &PreparedConfig) -> Result<(), CmdError> {
    let parent = prepared.path.parent().ok_or_else(|| {
        CmdError::click(format!(
            "{} has no parent directory",
            prepared.path.display()
        ))
    })?;
    fs::create_dir_all(parent)?;
    let backup = path_with_suffix(&prepared.path, ".pre-stado-recovery")?;
    if prepared.path.exists() && !backup.exists() {
        fs::copy(&prepared.path, &backup)?;
    }
    let temp = parent.join(format!(
        ".{}.recovery-{}.tmp",
        prepared
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stado-config"),
        uuid::Uuid::new_v4()
    ));
    let write_result = (|| -> Result<(), std::io::Error> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&prepared.bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &prepared.path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result?;
    println!(
        "  wrote {} (rollback: {})",
        prepared.path.display(),
        backup.display()
    );
    Ok(())
}

fn path_with_suffix(path: &Path, suffix: &str) -> Result<PathBuf, CmdError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CmdError::click(format!("invalid config path {}", path.display())))?;
    Ok(path.with_file_name(format!("{name}{suffix}")))
}

async fn install_remote_configs(
    services: &[ResolvedService],
    bytes: &[u8],
) -> Result<(), CmdError> {
    let mut destinations: BTreeMap<(String, String), ComputeTarget> = BTreeMap::new();
    for resolved in services {
        destinations.insert(
            (
                resolved.reference.host.clone(),
                resolved.config_path.clone(),
            ),
            resolved.target.clone(),
        );
    }
    let runner = production_runner();
    for ((host, path), target) in destinations {
        let script = remote_config_script(&path, bytes);
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(deploy_error)?;
        if !output.ok() || !output.stdout.contains("STADO_RECOVERY_CONFIG\tinstalled\t") {
            return Err(CmdError::click(format!(
                "{host}: config cutover failed: {}",
                host_channel::last_error_line(&output, "missing recovery config marker")
            )));
        }
        println!("  {host}: wrote {path}");
    }
    Ok(())
}

fn remote_config_script(path: &str, bytes: &[u8]) -> String {
    let path_b64 = STANDARD.encode(path.as_bytes());
    let body_b64 = STANDARD.encode(bytes);
    format!(
        r#"set -eu
umask 077
case "$(/usr/bin/uname -s)" in
  Darwin) decode_flag=-D ;;
  *) decode_flag=--decode ;;
esac
config_path=$(printf '%s' '{path_b64}' | /usr/bin/base64 "$decode_flag")
case "$config_path" in
  /*) ;;
  *) printf 'config path is not absolute\n' >&2; exit 64 ;;
esac
parent=$(/usr/bin/dirname "$config_path")
/bin/mkdir -p "$parent"
tmp="$config_path.recovery.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
printf '%s' '{body_b64}' | /usr/bin/base64 "$decode_flag" > "$tmp"
/bin/chmod 600 "$tmp"
if [ -f "$config_path" ] && [ ! -f "$config_path.pre-stado-recovery" ]; then
  /bin/cp -p "$config_path" "$config_path.pre-stado-recovery"
fi
/bin/mv -f "$tmp" "$config_path"
trap - EXIT HUP INT TERM
printf 'STADO_RECOVERY_CONFIG\tinstalled\t%s\n' "$config_path"
"#
    )
}

fn print_plan(
    args: &RecoveryMigrateArgs,
    source: &Endpoint,
    destination: &Endpoint,
    prepared: &PreparedConfig,
) {
    println!("DRY RUN — no network call, file write, service action, or billing change");
    println!("source:      {}", source.describe());
    println!("destination: {}", destination.describe());
    println!("config:      {}", prepared.path.display());
    println!("providers:   {}", args.enable_providers.join(", "));
    println!("writers:     {}", display_refs(&args.writers));
    println!("activate:    {}", display_refs(&args.activate));
    println!("resume:      {}", args.resume);
    println!(
        "billing:     {}",
        if args.manage_gcp_billing {
            "bounded GCP window"
        } else {
            "unchanged"
        }
    );
    println!("steps: destination fence+drain -> source billing window -> source fence+drain -> stop writers -> canonical copy -> full body+metadata verify -> close billing -> atomic config cutover -> selected restart -> optional resume");
}

fn display_refs(references: &[ServiceRef]) -> String {
    if references.is_empty() {
        return "none".to_string();
    }
    references
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}

async fn ensure_billing_disabled(project: &str) -> Result<(), CmdError> {
    let value = gcp_billing_request(reqwest::Method::GET, project, None).await?;
    match value.get("billingEnabled").and_then(Value::as_bool) {
        Some(false) => Ok(()),
        Some(true) => Err(CmdError::click(format!(
            "GCP billing for {project} is already enabled; refusing to claim ownership of a window this command did not open"
        ))),
        None => Err(CmdError::click(format!(
            "Cloud Billing response did not explicitly confirm billingEnabled=false: {value}"
        ))),
    }
}

async fn update_gcp_billing(project: &str, account: &str) -> Result<(), CmdError> {
    let value = gcp_billing_request(
        reqwest::Method::PUT,
        project,
        Some(json!({"billingAccountName": account})),
    )
    .await?;
    let enabled = value
        .get("billingEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let landed = value
        .get("billingAccountName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !enabled || landed != account {
        return Err(CmdError::click(format!(
            "Cloud Billing did not confirm the requested account: {value}"
        )));
    }
    Ok(())
}

async fn close_billing_window(project: &str) -> Result<(), CmdError> {
    let mut last = None;
    let attempts = "GCP".len();
    for attempt in usize::MIN..attempts {
        match gcp_billing_request(
            reqwest::Method::PUT,
            project,
            Some(json!({"billingAccountName": ""})),
        )
        .await
        {
            Ok(value) if value.get("billingEnabled").and_then(Value::as_bool) == Some(false) => {
                return Ok(())
            }
            Ok(value) => {
                last = Some(format!(
                    "Cloud Billing did not explicitly confirm billingEnabled=false: {value}"
                ))
            }
            Err(error) => last = Some(error.to_string()),
        }
        if attempt.saturating_add(usize::from(true)) < attempts {
            tokio::time::sleep(Duration::from_secs("ok".len() as u64)).await;
        }
    }
    Err(CmdError::click(last.unwrap_or_else(|| {
        "unknown Cloud Billing close failure".to_string()
    })))
}

async fn gcp_billing_request(
    method: reqwest::Method,
    project: &str,
    body: Option<Value>,
) -> Result<Value, CmdError> {
    let auth = crate::skarbiec::gcp_provider()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let token = auth
        .token(&[BILLING_SCOPE])
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let url = format!("{CLOUD_BILLING_BASE}/{project}/billingInfo");
    let client = reqwest::Client::new();
    let mut request = client.request(method, &url).bearer_auth(token.as_str());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "Cloud Billing HTTP {status}: {text}"
        )));
    }
    Ok(serde_json::from_str(&text)?)
}

fn combine_billing_error(open: CmdError, close: Result<(), CmdError>) -> CmdError {
    match close {
        Ok(()) => CmdError::click(format!("could not open the GCP billing window: {open}; a defensive detach request succeeded")),
        Err(close) => CmdError::click(format!("could not open the GCP billing window: {open}; CRITICAL: the defensive detach request also failed: {close}")),
    }
}
