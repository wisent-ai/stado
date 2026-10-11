//! `stado schedule ...` — manage recurring (cron) jobs.
//!
//! Port of the `schedule` click group in `stado/cli.py`. Output formats
//! (created/paused/resumed/fired lines, the list table, `show` JSON) are
//! byte-compatible with the Python CLI.

use chrono::Utc;

use crate::models::isoformat_utc;
use crate::queue::JobStorage;
use crate::schedules::{self, compute_next_due, cron_is_valid, read_schedule, Schedule};

use crate::cli::CmdError;

mod create;
pub use create::create;

/// `schedule list`: all schedules, sorted by next run (paused last).
pub async fn list(json: bool) -> Result<(), CmdError> {
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
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&scheds).map_err(CmdError::from)?
        );
        return Ok(());
    }
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
        // Whole values; the columns pad short ones and the command runs last.
        let cmd = s.command.split_whitespace().collect::<Vec<_>>().join(" ");
        let tz = &s.tz;
        let next = if s.next_due_at.is_empty() {
            "-"
        } else {
            s.next_due_at.as_str()
        };
        println!(
            "{:<14} {:<3} {:<16} {:<14} {:<28} {:>5} {}",
            s.schedule_id, en, s.cron, tz, next, s.fire_count, cmd
        );
    }
    println!("\n{count} schedule(s)");
    Ok(())
}

/// The caller named a schedule the store does not hold.
fn unknown_schedule(schedule_id: &str) -> CmdError {
    CmdError::refused(format!("schedule {schedule_id} not found"))
}

/// `schedule show ID [--json]`: one schedule's fields as text, or its full
/// persisted record as JSON — both from the same record. A schedule still
/// waiting past its next run carries `overdue`: why it has not fired, read
/// against the coordinator's last recorded schedule sweep.
pub async fn show(schedule_id: &str, json: bool) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let Some(s) = read_schedule(&store, schedule_id)
        .await?
        .filter(|schedule| !schedule.deleted)
    else {
        return Err(unknown_schedule(schedule_id));
    };
    let mut record: serde_json::Value = serde_json::from_str(&s.to_json())?;
    let sweep = schedules::last_sweep(&store).await?;
    if let Some(reason) = schedules::overdue(&s, sweep.as_ref(), Utc::now()) {
        record["overdue"] = serde_json::Value::String(reason);
    }
    if let Some(sweep) = sweep {
        record["last_schedule_sweep"] = serde_json::to_value(sweep)?;
    }
    crate::cli::print_answer(&record, json)
}

/// `schedule rm ID`: delete a schedule (does not affect jobs it already
/// submitted).
pub async fn rm(schedule_id: &str) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    if schedules::delete_schedule(&store, schedule_id).await? {
        println!("deleted schedule {schedule_id}");
        Ok(())
    } else {
        Err(unknown_schedule(schedule_id))
    }
}

/// `schedule edit ID [--command] [--cron] [--tz]`: change what a schedule
/// submits or when. Nothing given is a usage error; an invalid cron or a
/// timezone the cron cannot be computed in is refused before anything is
/// written. An enabled schedule's next run is recomputed from now unless an
/// occurrence is already leased.
pub async fn edit(
    schedule_id: &str,
    command: Option<&str>,
    cron: Option<&str>,
    tz: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    if command.is_none() && cron.is_none() && tz.is_none() {
        return Err(CmdError::usage(
            "schedule edit needs at least one of --command, --cron or --tz",
        ));
    }
    if let Some(cron) = cron.filter(|cron| !cron_is_valid(cron)) {
        return Err(CmdError::usage(format!(
            "invalid cron expression: '{cron}'"
        )));
    }
    let store = JobStorage::new().await?;
    let Some(current) = read_schedule(&store, schedule_id).await? else {
        return Err(unknown_schedule(schedule_id));
    };
    let new_cron = cron.unwrap_or(&current.cron).to_string();
    let new_tz = tz.unwrap_or(&current.tz).to_string();
    let next_due = compute_next_due(&new_cron, Utc::now(), &new_tz)
        .map_err(|exc| CmdError::usage(format!("could not compute next run ({new_tz}): {exc}")))?;
    let next_due = isoformat_utc(next_due);
    let edited = schedules::edit_schedule(&store, schedule_id, |sched| {
        if let Some(command) = command {
            sched.command = command.to_string();
        }
        sched.cron = new_cron.clone();
        sched.tz = new_tz.clone();
        if sched.enabled && sched.pending_occurrence.is_none() {
            sched.next_due_at = next_due.clone();
        }
    })
    .await?
    .ok_or_else(|| unknown_schedule(schedule_id))?;
    if json {
        println!("{}", edited.to_json());
    } else {
        println!("edited schedule {schedule_id}");
        println!("  cron:     {}  ({})", edited.cron, edited.tz);
        println!("  next run: {}", edited.next_due_at);
    }
    Ok(())
}

/// Python `_set_enabled`.
async fn set_enabled(schedule_id: &str, enabled: bool) -> Result<Schedule, CmdError> {
    let store = JobStorage::new().await?;
    let Some(s) = read_schedule(&store, schedule_id).await? else {
        return Err(unknown_schedule(schedule_id));
    };
    let next_due = if enabled {
        Some(isoformat_utc(
            compute_next_due(&s.cron, Utc::now(), &s.tz)
                .map_err(|exc| CmdError::declaration(exc.to_string()))?,
        ))
    } else {
        None
    };
    schedules::set_schedule_enabled(&store, schedule_id, enabled, next_due.as_deref())
        .await?
        .ok_or_else(|| unknown_schedule(schedule_id))
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

/// `schedule run ID --retry-token TOKEN`: fire a schedule exactly once for
/// the caller-retained token, regardless of its next run time.
pub async fn run(schedule_id: &str, retry_token: &str, json: bool) -> Result<(), CmdError> {
    if retry_token.trim().is_empty() {
        return Err(CmdError::usage("--retry-token must not be empty"));
    }
    let store = JobStorage::new().await?;
    if read_schedule(&store, schedule_id).await?.is_none() {
        return Err(unknown_schedule(schedule_id));
    }
    let job = schedules::fire_schedule_now(&store, schedule_id, retry_token, Utc::now())
        .await?
        .ok_or_else(|| {
            CmdError::refused(format!(
                "schedule {schedule_id} occurrence is being submitted by another owner"
            ))
        })?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "schedule_id": schedule_id,
                "retry_token": retry_token,
                "job_id": job.job_id,
                "run_id": job.run_id,
            })
        );
    } else {
        println!(
            "fired {schedule_id} -> job {} (run {})",
            job.job_id, job.run_id
        );
    }
    Ok(())
}
