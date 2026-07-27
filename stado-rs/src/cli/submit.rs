//! `stado submit` — port of the `submit` command in `stado/cli.py`.

use clap::Args;
use serde_json::{Map, Value};

use crate::profiles;
use crate::queue::submit::{submit_batch, SubmitOptions};

use super::CmdError;

#[derive(Args, Debug)]
pub struct SubmitArgs {
    /// Shell command the job runs.
    command: String,

    /// Preferred provider (gcp/azure/aws/local). With --any-provider this is just a hint.
    #[arg(long, default_value = "gcp")]
    provider: String,
    /// File with commands
    #[arg(long)]
    batch: Option<String>,
    /// Dispatch on Spot/Preemptible GPUs (cheaper, can be preempted).
    #[arg(long = "spot", overrides_with = "no_spot")]
    spot: bool,
    /// Do not dispatch on Spot/Preemptible GPUs (default).
    #[arg(long = "no-spot", overrides_with = "spot")]
    no_spot: bool,
    /// Hard cap on $/hour for the chosen accelerator. 0 = no cap.
    #[arg(long, default_value_t = 0.0)]
    max_cost_per_hour: f64,
    /// If true (default), any consumer with capacity can claim.
    #[arg(long = "any-provider", overrides_with = "pin_provider")]
    any_provider: bool,
    /// If --pin-provider, only the named --provider is allowed.
    #[arg(long = "pin-provider", overrides_with = "any_provider")]
    pin_provider: bool,
    /// Higher = scheduled first within FIFO bucket.
    #[arg(long, default_value_t = 0)]
    priority: i64,
    /// Optional git URL to clone before running command (no auth).
    #[arg(long, default_value = "")]
    repo: String,
    /// Override cloned-repo dir; default = repo basename.
    #[arg(long, default_value = "")]
    repo_workdir: String,
    /// pip extras to install on the clone; empty skips install.
    #[arg(long, default_value = "train")]
    repo_extras: String,
    /// Pin the accelerator label (e.g. 'nvidia-l4', 'nvidia-a100-80gb').
    /// Skips the --model regex inference. Resolves machine_type from
    /// GPU_SIZING unless --machine-type is also passed.
    #[arg(long, default_value = "")]
    gpu_type: String,
    /// Caller-declared VRAM (GB). Picks the smallest SKU whose tier >= this value.
    /// Skips the --model regex inference.
    #[arg(long, default_value_t = 0)]
    vram_gb: i64,
    /// Pin the GCE/Azure machine type verbatim (e.g. 'g2-standard-8').
    /// Use for SKUs not in the wisent-compute catalog.
    #[arg(long, default_value = "")]
    machine_type: String,
    /// Shell snippet placed before the command in the SAME bash shell.
    /// Use to export env vars (LD_LIBRARY_PATH, CUDA_VISIBLE_DEVICES, etc.)
    /// that the command will see.
    #[arg(long, default_value = "")]
    pre_command: String,
    /// Comma-separated apt package list. Installed via sudo apt-get on
    /// cloud-kind agents only — local-kind agents refuse the job for safety.
    #[arg(long, default_value = "")]
    apt: String,
    /// Additional gs:// destination for job output. Additive — canonical
    /// status/<id>/output/ path is always written too.
    #[arg(long, default_value = "")]
    output_uri: String,
    /// Shell command that must exit 0 after the job succeeds; non-zero
    /// reverses COMPLETED->FAILED. Catches silent-success failure modes.
    #[arg(long, default_value = "")]
    verify: String,
    /// Claim the WHOLE GPU. Agent only claims this job on an
    /// empty slot and refuses to admit any other job while it runs.
    /// Use for diffusion training / full-finetunes whose peak VRAM
    /// can't be safely co-tenanted.
    #[arg(long)]
    exclusive: bool,
    /// Background job: the local agent may EVICT this slot for a
    /// strictly-higher-priority queued job that doesn't otherwise
    /// fit. Requires --on-yield. The agent runs that hook (with
    /// WC_JOB_PID set), waits --yield-grace, then requeues the job
    /// (resumes from wherever the hook saved state).
    #[arg(long)]
    yieldable: bool,
    /// Save-and-sync command run when the agent yields this job.
    /// Responsible for telling the job to stop, persisting state
    /// + artifacts (server/GCS/HF), and letting it exit. Required
    ///   with --yieldable.
    #[arg(long, default_value = "")]
    on_yield: String,
    /// Seconds the --on-yield hook + clean exit get before the
    /// agent SIGKILLs the process group (default 120).
    #[arg(long, default_value_t = 120)]
    yield_grace: i64,
    /// Pinned artifact input as NAME=TYPE/NAMESPACE/NAME@VERSION_OR_ALIAS.
    #[arg(long = "input-artifact")]
    input_artifacts: Vec<String>,
    /// Apply a named profile from the bundled profiles dir (or
    /// $WC_PROFILES_DIR). CLI flags override profile fields.
    /// Run `stado profiles` to list available profiles.
    #[arg(long, default_value = "")]
    profile: String,
    /// Hard-pin this job to one consumer: a registry target
    /// name (resolved to kind-hostname) or a raw consumer_id.
    /// Only that consumer may claim the job; the makespan
    /// matcher never reassigns it.
    #[arg(long, default_value = "")]
    pinned_host: String,
}

/// Python `_resolve_input_artifacts`: validates the NAME=REF shape and
/// name safety, then resolves each ref through the artifacts registry at
/// submit time (aliases resolve to their immutable version) into
/// `resolved_input_artifacts` entries of `{"ref", "uri",
/// "manifest_sha256"}` — exactly the maps `cli.py` threads into the job.
async fn resolve_input_artifacts(
    values: &[String],
) -> Result<(Map<String, Value>, Map<String, Value>), CmdError> {
    let name_re = regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_-]{0,63}$").expect("static regex compiles");
    // The registry is built lazily: with no --input-artifact flags there is
    // nothing to resolve (Python constructs JobStorage eagerly, but its
    // constructor performs no I/O either).
    let registry = if values.is_empty() {
        None
    } else {
        Some(
            crate::artifacts::ArtifactRegistry::new()
                .await
                .map_err(|exc| CmdError::click(exc.to_string()))?,
        )
    };
    let mut requested = Map::new();
    let mut resolved = Map::new();
    for value in values {
        let Some((name, reference)) = value.split_once('=') else {
            return Err(CmdError::click(format!("--input-artifact must be NAME=REF: '{value}'")));
        };
        if !name_re.is_match(name) {
            return Err(CmdError::click(format!("artifact input name is unsafe: '{name}'")));
        }
        if requested.contains_key(name) {
            return Err(CmdError::click(format!("duplicate artifact input name: {name}")));
        }
        let manifest = registry
            .as_ref()
            .expect("registry exists when values are non-empty")
            .resolve_manifest(&crate::artifacts_models::ArtifactRef::parse(reference)?)
            .await?;
        let primary = manifest
            .locations
            .iter()
            .find(|location| location.role == "primary")
            .ok_or_else(|| {
                CmdError::click(format!("artifact has no primary location: {}", manifest.ref_))
            })?;
        requested.insert(name.to_string(), Value::from(reference));
        resolved.insert(
            name.to_string(),
            Value::Object(Map::from_iter([
                ("ref".into(), Value::from(manifest.ref_.to_string())),
                ("uri".into(), Value::from(primary.uri.clone())),
                (
                    "manifest_sha256".into(),
                    Value::from(manifest.verification.manifest_sha256.clone()),
                ),
            ])),
        );
    }
    Ok((requested, resolved))
}

/// The submit kwargs the CLI passes, as a JSON map keyed by the Python
/// kwarg names — the exact input `profiles.merge_into_kwargs` expects.
fn cli_kwargs_json(args: &SubmitArgs, apt_list: &[String], spot: bool, any_provider: bool) -> Map<String, Value> {
    Map::from_iter([
        ("gpu_type".into(), Value::from(args.gpu_type.as_str())),
        ("vram_gb".into(), Value::from(args.vram_gb)),
        ("machine_type".into(), Value::from(args.machine_type.as_str())),
        (
            "apt_packages".into(),
            Value::Array(apt_list.iter().map(|p| Value::from(p.as_str())).collect()),
        ),
        ("pre_command".into(), Value::from(args.pre_command.as_str())),
        ("repo".into(), Value::from(args.repo.as_str())),
        ("repo_workdir".into(), Value::from(args.repo_workdir.as_str())),
        ("repo_extras".into(), Value::from(args.repo_extras.as_str())),
        ("output_uri".into(), Value::from(args.output_uri.as_str())),
        ("verify_command".into(), Value::from(args.verify.as_str())),
        ("exclusive".into(), Value::from(args.exclusive)),
        ("priority".into(), Value::from(args.priority)),
        ("preemptible".into(), Value::from(spot)),
        ("max_cost_per_hour_usd".into(), Value::from(args.max_cost_per_hour)),
        ("provider".into(), Value::from(args.provider.as_str())),
        ("pin_to_provider".into(), Value::from(!any_provider)),
    ])
}

fn get_str(map: &Map<String, Value>, key: &str) -> String {
    map.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn get_i64(map: &Map<String, Value>, key: &str) -> i64 {
    map.get(key).and_then(Value::as_i64).unwrap_or_default()
}

fn get_f64(map: &Map<String, Value>, key: &str) -> f64 {
    map.get(key).and_then(Value::as_f64).unwrap_or_default()
}

fn get_bool(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).and_then(Value::as_bool).unwrap_or_default()
}

fn get_str_list(map: &Map<String, Value>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

pub async fn run(args: &SubmitArgs) -> Result<(), CmdError> {
    if args.yieldable && args.on_yield.trim().is_empty() {
        return Err(CmdError::click(
            "--yieldable requires --on-yield '<command>': a yieldable job must \
             declare how it saves state and steps aside. There is no silent \
             kill-and-restart path.",
        ));
    }
    let mut apt_list: Vec<String> =
        args.apt.split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).collect();
    let (requested_artifacts, resolved_artifacts) =
        resolve_input_artifacts(&args.input_artifacts).await?;

    let mut provider = args.provider.clone();
    let mut gpu_type = args.gpu_type.clone();
    let mut vram_gb = args.vram_gb;
    let mut machine_type = args.machine_type.clone();
    let mut pre_command = args.pre_command.clone();
    let mut repo = args.repo.clone();
    let mut repo_workdir = args.repo_workdir.clone();
    let mut repo_extras = args.repo_extras.clone();
    let mut output_uri = args.output_uri.clone();
    let mut verify_command = args.verify.clone();
    let mut exclusive = args.exclusive;
    let mut priority = args.priority;
    let mut spot = args.spot && !args.no_spot;
    let mut max_cost_per_hour = args.max_cost_per_hour;
    let mut any_provider = !args.pin_provider;

    // Profile merge — CLI args win on conflict. The submit kwargs map is
    // built from the clap values (which all have known defaults), then
    // merge_into_kwargs adopts profile fields wherever the CLI value
    // matches the wisent-compute default.
    if !args.profile.is_empty() {
        let profile = profiles::load_profile(&args.profile).map_err(|exc| CmdError::click(exc.to_string()))?;
        let merged = profiles::merge_into_kwargs(&profile, &cli_kwargs_json(args, &apt_list, spot, any_provider));
        gpu_type = get_str(&merged, "gpu_type");
        vram_gb = get_i64(&merged, "vram_gb");
        machine_type = get_str(&merged, "machine_type");
        apt_list = get_str_list(&merged, "apt_packages");
        pre_command = get_str(&merged, "pre_command");
        repo = get_str(&merged, "repo");
        repo_workdir = get_str(&merged, "repo_workdir");
        repo_extras = get_str(&merged, "repo_extras");
        output_uri = get_str(&merged, "output_uri");
        verify_command = get_str(&merged, "verify_command");
        exclusive = get_bool(&merged, "exclusive");
        priority = get_i64(&merged, "priority");
        spot = get_bool(&merged, "preemptible");
        max_cost_per_hour = get_f64(&merged, "max_cost_per_hour_usd");
        provider = get_str(&merged, "provider");
        any_provider = !get_bool(&merged, "pin_to_provider");
        let description = profile.get("description").and_then(Value::as_str).unwrap_or("");
        let description: String = description.chars().take(80).collect();
        println!("Profile '{}' applied: {description}", args.profile);
    }

    let mut pinned_host = args.pinned_host.clone();
    if !pinned_host.is_empty() {
        let registry = crate::targets::load_bundled_registry()
            .map_err(|exc| CmdError::click(exc.to_string()))?;
        if let Some(target) = registry.lookup(&pinned_host) {
            if target.hostnames.is_empty() {
                return Err(CmdError::click(format!(
                    "--pinned-host target '{pinned_host}' has no hostnames[] \
                     in the registry; cannot derive its consumer_id."
                )));
            }
            pinned_host = format!("{}-{}", target.kind, target.hostnames[0]);
        }
        println!("Job pinned to consumer: {pinned_host}");
    }

    let commands: Vec<String> = match &args.batch {
        Some(batch_file) => std::fs::read_to_string(batch_file)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect(),
        None => vec![args.command.clone()],
    };
    let batch_id = format!("batch-{}", chrono::Utc::now().timestamp());

    let options = SubmitOptions {
        provider,
        batch_id: batch_id.clone(),
        bucket: crate::config::bucket().to_string(),
        preemptible: spot,
        max_cost_per_hour_usd: max_cost_per_hour,
        pin_to_provider: !any_provider,
        priority,
        repo,
        repo_workdir,
        repo_extras,
        gpu_type,
        vram_gb,
        machine_type,
        pre_command,
        apt_packages: apt_list.clone(),
        output_uri: output_uri.clone(),
        verify_command: verify_command.clone(),
        exclusive,
        yieldable: args.yieldable,
        yield_command: args.on_yield.clone(),
        yield_grace_seconds: args.yield_grace,
        pinned_host,
        input_artifacts: requested_artifacts,
        resolved_input_artifacts: resolved_artifacts,
        ..Default::default()
    };

    let jobs = submit_batch(&commands, &options).await?;
    let n = jobs.len();
    if n == 1 {
        // Single job: echo its id so callers (probierz bridge) watch the
        // job itself instead of guessing from the batch id.
        println!("Job ID: {}", jobs[0].job_id);
    }
    println!("  submitted {}/{} jobs", n, commands.len());
    let mode = if super::api_key().is_empty() { "GCS" } else { "API" };
    let mut flags: Vec<String> = Vec::new();
    if options.preemptible {
        flags.push("spot".into());
    }
    if options.max_cost_per_hour_usd > 0.0 {
        flags.push(format!("cap=${:.2}/hr", options.max_cost_per_hour_usd));
    }
    if options.pin_to_provider {
        flags.push(format!("pinned={}", options.provider));
    }
    if options.priority != 0 {
        flags.push(format!("priority={}", options.priority));
    }
    if !options.gpu_type.is_empty() {
        flags.push(format!("gpu={}", options.gpu_type));
    }
    if options.vram_gb != 0 {
        flags.push(format!("vram={}G", options.vram_gb));
    }
    if !options.machine_type.is_empty() {
        flags.push(format!("mt={}", options.machine_type));
    }
    if !apt_list.is_empty() {
        flags.push(format!("apt={}", apt_list.join(",")));
    }
    if !options.pre_command.is_empty() {
        flags.push("pre_cmd".into());
    }
    if !output_uri.is_empty() {
        flags.push(format!("out={output_uri}"));
    }
    if !verify_command.is_empty() {
        flags.push("verify".into());
    }
    let flag_str = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join(", ")) };
    println!("\nSubmitted {} job(s) via {mode}{flag_str}. Batch: {batch_id}", commands.len());
    Ok(())
}
