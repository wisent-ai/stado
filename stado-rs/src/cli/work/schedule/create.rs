//! `stado schedule create`: declare a recurring schedule, and leave one
//! declared the same way under a caller-retained id as it is.

use chrono::Utc;

use crate::models::isoformat_utc;
use crate::queue::JobStorage;
use crate::schedules::{
    self, compute_next_due, cron_is_valid, generate_schedule_id, read_schedule, Schedule,
};

use crate::cli::{CmdError, ScheduleCreateArgs};

/// `$USER` or `$LOGNAME` (Python `os.environ.get("USER", "") or ...`).
fn created_by() -> String {
    let user = std::env::var("USER").unwrap_or_default();
    if !user.is_empty() {
        return user;
    }
    std::env::var("LOGNAME").unwrap_or_default()
}

/// `schedule create COMMAND --cron EXPR [...]`: create a recurring schedule
/// that submits COMMAND on a cron schedule.
pub async fn create(args: &ScheduleCreateArgs) -> Result<(), CmdError> {
    if !cron_is_valid(&args.cron) {
        return Err(CmdError::usage(format!(
            "invalid cron expression: '{}'",
            args.cron
        )));
    }
    if let Some(boot_disk_gb) = args.boot_disk_gb.filter(|value| !value.is_positive()) {
        return Err(CmdError::usage(format!(
            "--boot-disk-gb must be a positive whole number of GB, not {boot_disk_gb}"
        )));
    }
    if let Some(seconds) = args
        .runtime_seconds_estimate
        .filter(|value| !(value.is_normal() && value.is_sign_positive()))
    {
        return Err(CmdError::usage(format!(
            "--runtime-seconds-estimate must be a positive number of seconds, not {seconds}"
        )));
    }
    let apt_list: Vec<String> = args
        .apt
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let secret_env = crate::cli::submit::parse_secret_env(&args.secret_env)?;
    let now = Utc::now();
    let next_due = compute_next_due(&args.cron, now, &args.tz).map_err(|exc| {
        CmdError::usage(format!("could not compute next run ({}): {exc}", args.tz))
    })?;
    let sid = args
        .id
        .map(|id| format!("sch-{}", id.simple()))
        .unwrap_or_else(generate_schedule_id);
    let mut sched = Schedule::new(&sid, &args.cron, &args.command);
    sched.tz = args.tz.clone();
    sched.enabled = !args.disabled;
    sched.provider = args.provider.clone();
    sched.pin_to_provider = args.pin_provider;
    sched.preemptible = args.spot;
    sched.max_cost_per_hour_usd = args.max_cost_per_hour;
    sched.priority = args.priority;
    sched.gpu_type = args.gpu_type.clone();
    sched.vram_gb = args.vram_gb;
    sched.machine_type = args.machine_type.clone();
    sched.pinned_host = crate::cli::submit::resolve_pinned_host(&args.pinned_host).await?;
    sched.repo = args.repo.clone();
    sched.repo_ref = args.repo_ref.clone();
    sched.repo_workdir = args.repo_workdir.clone();
    sched.repo_extras = args.repo_extras.clone();
    sched.pre_command = args.pre_command.clone();
    sched.apt_packages = apt_list;
    sched.output_uri = args.output_uri.clone();
    sched.verify_command = args.verify.clone();
    sched.exclusive = args.exclusive;
    sched.boot_disk_gb = args.boot_disk_gb;
    sched.runtime_seconds_estimate = args.runtime_seconds_estimate;
    sched.secret_env = secret_env;
    sched.overlap_policy = args.overlap_policy.clone();
    sched.created_by = created_by();
    sched.next_due_at = isoformat_utc(next_due);
    let store = JobStorage::new().await?;
    if args.id.is_some() {
        if let Some(existing) = read_schedule(&store, &sid).await? {
            // A caller-retained identity makes the declaration safe to repeat:
            // the same declaration again is already done, and a different one
            // is refused naming what differs, so an install can declare its
            // schedule on every run without ever overwriting another's.
            let differs = declaration_differences(&existing, &sched);
            if existing.deleted || !differs.is_empty() {
                return Err(CmdError::refused(format!(
                    "schedule {sid} already exists{}; change it with `stado schedule edit {sid}` \
                     or remove it with `stado schedule rm {sid}`",
                    if existing.deleted {
                        " as a deleted record".to_string()
                    } else {
                        format!(" with a different {}", differs.join(", "))
                    }
                )));
            }
            if args.json {
                println!("{}", existing.to_json());
            } else {
                println!("schedule {sid} is already declared as asked; nothing changed");
            }
            return Ok(());
        }
    }
    schedules::write_schedule(&store, &sched).await?;
    if args.json {
        println!("{}", sched.to_json());
        return Ok(());
    }
    let state = if sched.enabled { "enabled" } else { "DISABLED" };
    println!("created schedule {sid} ({state})");
    println!("  cron:     {}  ({})", args.cron, args.tz);
    println!("  next run: {}", sched.next_due_at);
    println!("  command:  {}", args.command);
    Ok(())
}

/// The declared fields in which `existing` differs from `wanted`: what the
/// schedule submits, when, where and with which secrets. Bookkeeping such as
/// its next run, its creator and its leases is not a declaration.
fn declaration_differences(existing: &Schedule, wanted: &Schedule) -> Vec<&'static str> {
    let mut differs = Vec::new();
    let mut compare = |name: &'static str, same: bool| {
        if !same {
            differs.push(name);
        }
    };
    compare("command", existing.command == wanted.command);
    compare("cron", existing.cron == wanted.cron);
    compare("timezone", existing.tz == wanted.tz);
    compare("enabled state", existing.enabled == wanted.enabled);
    compare("pinned host", existing.pinned_host == wanted.pinned_host);
    compare("provider", existing.provider == wanted.provider);
    compare(
        "secret environment",
        existing.secret_env == wanted.secret_env,
    );
    compare(
        "repository",
        existing.repo == wanted.repo && existing.repo_ref == wanted.repo_ref,
    );
    compare("pre-command", existing.pre_command == wanted.pre_command);
    compare(
        "overlap policy",
        existing.overlap_policy == wanted.overlap_policy,
    );
    differs
}
