//! `stado schedule ...` — manage recurring (cron) jobs.
//!
//! Port of the `schedule` click group in `stado/cli.py`. Output formats
//! (created/paused/resumed/fired lines, the list table, `show` JSON) are
//! byte-compatible with the Python CLI.

use chrono::Utc;

use crate::models::isoformat_utc;
use crate::queue::runs::generate_run_id;
use crate::queue::submit::{submit_job, SubmitOptions};
use crate::queue::JobStorage;
use crate::schedules::{
    self, compute_next_due, cron_is_valid, generate_schedule_id, read_schedule, Schedule,
};

use super::{CmdError, ScheduleCreateArgs};

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
        return Err(CmdError::click(format!(
            "invalid cron expression: '{}'",
            args.cron
        )));
    }
    let apt_list: Vec<String> = args
        .apt
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let secret_env = super::submit::parse_secret_env(&args.secret_env)?;
    let now = Utc::now();
    let next_due = compute_next_due(&args.cron, now, &args.tz).map_err(|exc| {
        CmdError::click(format!("could not compute next run ({}): {exc}", args.tz))
    })?;
    let sid = generate_schedule_id();
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
    sched.repo = args.repo.clone();
    sched.repo_ref = args.repo_ref.clone();
    sched.repo_workdir = args.repo_workdir.clone();
    sched.repo_extras = args.repo_extras.clone();
    sched.pre_command = args.pre_command.clone();
    sched.apt_packages = apt_list;
    sched.output_uri = args.output_uri.clone();
    sched.verify_command = args.verify.clone();
    sched.exclusive = args.exclusive;
    sched.secret_env = secret_env;
    sched.overlap_policy = args.overlap_policy.clone();
    sched.created_by = created_by();
    sched.next_due_at = isoformat_utc(next_due);
    let store = JobStorage::new().await?;
    schedules::write_schedule(&store, &sched).await?;
    let state = if sched.enabled { "enabled" } else { "DISABLED" };
    println!("created schedule {sid} ({state})");
    println!("  cron:     {}  ({})", args.cron, args.tz);
    println!("  next run: {}", sched.next_due_at);
    println!(
        "  command:  {}",
        args.command.chars().take(80).collect::<String>()
    );
    Ok(())
}

/// `schedule list`: all schedules, sorted by next run (paused last).
pub async fn list() -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let mut scheds = schedules::list_schedules(&store).await?;
    // Python sort key: s.next_due_at or "~" — plain string sort, paused
    // ("" → "~") last.
    scheds.sort_by_key(|s| {
        if s.next_due_at.is_empty() {
            "~".to_string()
        } else {
            s.next_due_at.clone()
        }
    });
    if scheds.is_empty() {
        println!("(no schedules)");
        return Ok(());
    }
    println!(
        "{:<14} {:<3} {:<16} {:<14} {:<28} {:>5} COMMAND",
        "ID", "EN", "CRON", "TZ", "NEXT RUN (UTC)", "FIRED"
    );
    println!("{}", "-".repeat(120));
    let count = scheds.len();
    for s in &scheds {
        let en = if s.enabled { "Y" } else { "n" };
        let cmd = s.command.split_whitespace().collect::<Vec<_>>().join(" ");
        let cmd = if cmd.chars().count() > 34 {
            format!("{}…", cmd.chars().take(34).collect::<String>())
        } else {
            cmd
        };
        let tz: String = s.tz.chars().take(12).collect();
        let next: String = if s.next_due_at.is_empty() {
            "-".into()
        } else {
            s.next_due_at.chars().take(26).collect()
        };
        println!(
            "{:<14} {:<3} {:<16} {:<14} {:<28} {:>5} {}",
            s.schedule_id, en, s.cron, tz, next, s.fire_count, cmd
        );
    }
    println!("\n{count} schedule(s)");
    Ok(())
}

/// `schedule show ID`: print a schedule's full JSON.
pub async fn show(schedule_id: &str) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let Some(s) = read_schedule(&store, schedule_id).await? else {
        return Err(CmdError::click(format!("schedule {schedule_id} not found")));
    };
    println!("{}", s.to_json());
    Ok(())
}

/// `schedule rm ID`: delete a schedule (does not affect jobs it already
/// submitted).
pub async fn rm(schedule_id: &str) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    if schedules::delete_schedule(&store, schedule_id).await? {
        println!("deleted schedule {schedule_id}");
        Ok(())
    } else {
        Err(CmdError::click(format!("schedule {schedule_id} not found")))
    }
}

/// Python `_set_enabled`.
async fn set_enabled(schedule_id: &str, enabled: bool) -> Result<Schedule, CmdError> {
    let store = JobStorage::new().await?;
    let Some(mut s) = read_schedule(&store, schedule_id).await? else {
        return Err(CmdError::click(format!("schedule {schedule_id} not found")));
    };
    s.enabled = enabled;
    if enabled {
        // Recompute next_due from now so a long-paused schedule doesn't
        // fire immediately for a stale overdue slot.
        s.next_due_at = isoformat_utc(
            compute_next_due(&s.cron, Utc::now(), &s.tz)
                .map_err(|exc| CmdError::click(exc.to_string()))?,
        );
    }
    schedules::write_schedule(&store, &s).await?;
    Ok(s)
}

/// `schedule pause ID`: disable a schedule without deleting it.
pub async fn pause(schedule_id: &str) -> Result<(), CmdError> {
    set_enabled(schedule_id, false).await?;
    println!("paused {schedule_id}");
    Ok(())
}

/// `schedule resume ID`: re-enable a paused schedule (next run recomputed
/// from now).
pub async fn resume(schedule_id: &str) -> Result<(), CmdError> {
    let s = set_enabled(schedule_id, true).await?;
    println!("resumed {schedule_id}; next run {}", s.next_due_at);
    Ok(())
}

/// `schedule run ID`: fire a schedule once right now, regardless of its
/// next run time.
pub async fn run(schedule_id: &str) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let Some(mut s) = read_schedule(&store, schedule_id).await? else {
        return Err(CmdError::click(format!("schedule {schedule_id} not found")));
    };
    let run_id = generate_run_id();
    let options = SubmitOptions {
        bucket: crate::config::bucket().to_string(),
        run_id: run_id.clone(),
        schedule_id: schedule_id.to_string(),
        ..s.submit_options()
    };
    let job = submit_job(&s.command, &options).await?;
    s.last_fired_at = Some(isoformat_utc(Utc::now()));
    s.last_run_id = run_id.clone();
    s.last_job_id = job.job_id.clone();
    s.fire_count += 1;
    schedules::write_schedule(&store, &s).await?;
    println!("fired {schedule_id} -> job {} (run {run_id})", job.job_id);
    Ok(())
}
