//! The `stado submit` command itself: it validates the flag combinations,
//! resolves the pinned identities, folds the profile over the CLI kwargs,
//! assembles one [`SubmitOptions`] request, dispatches the batch and prints
//! the receipt the submission is keyed by.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cli::submit::identity::{
    parse_secret_env, resolve_input_artifacts, resolve_pinned_host,
};
use crate::cli::submit::receipt::{
    ResolvedExecutorReceipt, SubmissionJobReceipt, SubmissionReceipt,
};
use crate::cli::submit::request::kwargs::{
    cli_kwargs_json, get_bool, get_f64, get_i64, get_str, get_str_list,
};
use crate::cli::submit::SubmitArgs;
use crate::cli::CmdError;
use crate::profiles;
use crate::queue::submit::{
    submission_input_digest, submission_job_key, submission_source_digest, submit_batch,
    SubmitOptions,
};

pub async fn run(args: &SubmitArgs) -> Result<(), CmdError> {
    if args.yieldable && args.on_yield.trim().is_empty() {
        return Err(CmdError::click(
            "--yieldable requires --on-yield '<command>': a yieldable job must \
             declare how it saves state and steps aside. There is no silent \
             kill-and-restart path.",
        ));
    }
    let mut apt_list: Vec<String> = args
        .apt
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    let (requested_artifacts, resolved_artifacts) =
        resolve_input_artifacts(&args.input_artifacts).await?;
    let secret_env = parse_secret_env(&args.secret_env)?;

    let mut provider = args.provider.clone();
    let mut gpu_type = args.gpu_type.clone();
    let mut vram_gb = args.vram_gb;
    let mut machine_type = args.machine_type.clone();
    let mut pre_command = args.pre_command.clone();
    let mut repo = args.repo.clone();
    let mut repo_ref = args.repo_ref.clone();
    let mut repo_workdir = args.repo_workdir.clone();
    let mut repo_extras = args.repo_extras.clone();
    let mut output_uri = args.output_uri.clone();
    let mut verify_command = args.verify.clone();
    let mut exclusive = args.exclusive;
    let mut priority = args.priority;
    let mut deadline_at = args.deadline_at.clone();
    let mut spot = args.spot && !args.no_spot;
    let mut max_cost_per_hour = args.max_cost_per_hour;
    let mut any_provider = !args.pin_provider;

    // Profile merge — CLI args win on conflict. The submit kwargs map is
    // built from the clap values (which all have known defaults), then
    // merge_into_kwargs adopts profile fields wherever the CLI value
    // matches the wisent-compute default.
    if !args.profile.is_empty() {
        let profile = profiles::load_profile(&args.profile)
            .map_err(|exc| CmdError::click(exc.to_string()))?;
        let merged = profiles::merge_into_kwargs(
            &profile,
            &cli_kwargs_json(args, &apt_list, spot, any_provider),
        );
        gpu_type = get_str(&merged, "gpu_type");
        vram_gb = get_i64(&merged, "vram_gb");
        machine_type = get_str(&merged, "machine_type");
        apt_list = get_str_list(&merged, "apt_packages");
        pre_command = get_str(&merged, "pre_command");
        repo = get_str(&merged, "repo");
        repo_ref = get_str(&merged, "repo_ref");
        repo_workdir = get_str(&merged, "repo_workdir");
        repo_extras = get_str(&merged, "repo_extras");
        output_uri = get_str(&merged, "output_uri");
        verify_command = get_str(&merged, "verify_command");
        exclusive = get_bool(&merged, "exclusive");
        priority = get_i64(&merged, "priority");
        deadline_at = get_str(&merged, "deadline_at");
        spot = get_bool(&merged, "preemptible");
        max_cost_per_hour = get_f64(&merged, "max_cost_per_hour_usd");
        provider = get_str(&merged, "provider");
        any_provider = !get_bool(&merged, "pin_to_provider");
        let description = profile
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let description: String = description.chars().take(80).collect();
        println!("Profile '{}' applied: {description}", args.profile);
    }
    let deadline_at = if deadline_at.trim().is_empty() {
        None
    } else {
        let parsed = chrono::DateTime::parse_from_rfc3339(&deadline_at)
            .map_err(|error| CmdError::click(format!("--deadline-at must be RFC 3339: {error}")))?;
        let parsed = parsed.with_timezone(&chrono::Utc);
        if parsed <= chrono::Utc::now() {
            return Err(CmdError::click("--deadline-at must be in the future"));
        }
        Some(parsed.to_rfc3339())
    };

    let pinned_host = resolve_pinned_host(&args.pinned_host).await?;
    if !pinned_host.is_empty() {
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
    let batch_id = format!("run-{}", args.run_id);

    let options = SubmitOptions {
        provider,
        batch_id: batch_id.clone(),
        bucket: crate::config::bucket().to_string(),
        preemptible: spot,
        max_cost_per_hour_usd: max_cost_per_hour,
        pin_to_provider: !any_provider,
        priority,
        deadline_at: deadline_at.clone(),
        repo,
        repo_ref,
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
        run_id: args.run_id.clone(),
        secret_env,
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
    let mode = "Stado";
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
    if let Some(deadline) = &options.deadline_at {
        flags.push(format!("deadline={deadline}"));
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
    if !options.secret_env.is_empty() {
        flags.push(format!("secrets={}", options.secret_env.len()));
    }
    if !output_uri.is_empty() {
        flags.push(format!("out={output_uri}"));
    }
    if !verify_command.is_empty() {
        flags.push("verify".into());
    }
    let flag_str = if flags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", flags.join(", "))
    };
    println!(
        "\nSubmitted {} job(s) via {mode}{flag_str}. Batch: {batch_id}",
        commands.len()
    );
    let receipt_options = options.clone();
    let request_digest = jobs
        .first()
        .map(|job| job.submission_request_digest.clone())
        .ok_or_else(|| CmdError::click("durable submission returned no jobs"))?;
    let receipt = SubmissionReceipt {
        schema: "stado.submission-receipt.v3".into(),
        run_id: jobs
            .first()
            .map(|job| job.run_id.clone())
            .unwrap_or_else(|| options.run_id.clone()),
        request_digest: request_digest.clone(),
        source_digest: submission_source_digest(&receipt_options),
        input_digest: submission_input_digest(&commands, &receipt_options),
        repo: receipt_options.repo.clone(),
        repo_ref: receipt_options.repo_ref.clone(),
        source_revision: receipt_options.repo_ref.clone(),
        jobs: jobs
            .iter()
            .enumerate()
            .map(|(index, job)| SubmissionJobReceipt {
                command_index: index,
                command: job.command.clone(),
                command_digest: hex::encode(Sha256::digest(job.command.as_bytes())),
                job_key: submission_job_key(&request_digest, index, &job.command),
                job_id: job.job_id.clone(),
                output_uri: job.output_uri.clone(),
                pinned_host: job.pinned_host.clone(),
                resolved_executor: ResolvedExecutorReceipt {
                    provider: job.provider.clone(),
                    machine_type: job.machine_type.clone(),
                    gpu_type: job.gpu_type.clone(),
                    platform_os: job.platform_os.clone(),
                    architecture: job.architecture.clone(),
                },
                repo_ref: job.repo_ref.clone(),
                submission_request_digest: job.submission_request_digest.clone(),
            })
            .collect(),
    };
    println!("{}", serde_json::to_string(&receipt)?);
    Ok(())
}
