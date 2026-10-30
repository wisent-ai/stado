//! `stado schedule create`: declare a recurring schedule, and leave one
//! declared the same way under a caller-retained id as it is.

use chrono::Utc;

use crate::models::isoformat_utc;
use crate::queue::{JobStorage, StorageError};
use crate::schedules::{compute_next_due, cron_is_valid, generate_schedule_id, Schedule};

use super::declare::{declare, Declared};
use crate::cli::{CmdError, ScheduleCreateArgs};

/// Who declares the schedule, read as a job submission reads its submitter
/// (`USER`, `LOGNAME`, then the account in the user database); an account
/// none of them names is refused, where the schedule was stored created by
/// nobody.
fn created_by() -> Result<String, CmdError> {
    crate::queue::submit::submitter().map_err(|error| CmdError::usage(error.to_string()))
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
    // A caller-retained id is stored in the schedule-id form; without one a
    // new schedule gets a fresh id.
    let sid = match args.id {
        Some(id) => super::stored_id(&id.to_string()),
        None => generate_schedule_id(),
    };
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
    sched.overlap_policy = args.overlap_policy;
    sched.created_by = created_by()?;
    sched.next_due_at = isoformat_utc(next_due);
    let id_retained = args.id.is_some();
    let declared: Result<Declared, Declaring> = async {
        if args.host_store {
            // A host routine is kept on this host alone, so there is no
            // fleet declaration held here to deliver first, and no fleet
            // store to wait for.
            let store = crate::providers::local::host_routines::open()
                .map_err(|error| Declaring::Refused(CmdError::from(error)))?;
            return Ok(declare(&store, &sched, id_retained, args.converge)
                .await
                .map_err(|error| Declaring::Refused(CmdError::from(error)))??);
        }
        let store = JobStorage::new().await?;
        super::held::deliver(&store).await??;
        Ok(declare(&store, &sched, id_retained, args.converge).await??)
    }
    .await;
    let outcome = match declared {
        Ok(outcome) => outcome,
        // The store did not answer: the declaration is kept on this host and
        // the next schedule command that reaches the store writes it there.
        Err(Declaring::Unanswered(error)) => return held(&sched, args, id_retained, &error),
        Err(Declaring::Refused(error)) => return Err(error),
    };
    if args.json {
        println!("{}", outcome.schedule().to_json());
        return Ok(());
    }
    match &outcome {
        Declared::Created(_) => {
            let state = if sched.enabled { "enabled" } else { "DISABLED" };
            println!("created schedule {sid} ({state})");
            println!("  cron:     {}  ({})", args.cron, args.tz);
            println!("  next run: {}", sched.next_due_at);
            println!("  command:  {}", args.command);
        }
        Declared::Unchanged(_) => {
            println!("schedule {sid} is already declared as asked; nothing changed");
        }
        Declared::Converged(converged, differs) => {
            println!(
                "schedule {sid} brought to its declaration: changed {}",
                differs.join(", ")
            );
            println!("  next run: {}", converged.next_due_at);
        }
    }
    Ok(())
}

/// Keep `sched` on this host because the store answered `error` instead of
/// taking it, and say where and why.
fn held(
    sched: &Schedule,
    args: &ScheduleCreateArgs,
    id_retained: bool,
    error: &StorageError,
) -> Result<(), CmdError> {
    let path = super::held::hold(sched, id_retained, args.converge, error)?;
    if args.json {
        let report = serde_json::json!({
            "held": path.display().to_string(),
            "refusal": error.to_string(),
            "schedule": serde_json::from_str::<serde_json::Value>(&sched.to_json())?,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "held schedule {} on this host at {}: the fleet store did not answer ({error}); \
             the next schedule command that reaches it writes it there",
            sched.schedule_id,
            path.display()
        );
    }
    Ok(())
}

/// Why a declaration was not made: the store did not answer, so it is kept
/// on this host, or something else refused it.
enum Declaring {
    Unanswered(StorageError),
    Refused(CmdError),
}

impl From<CmdError> for Declaring {
    fn from(error: CmdError) -> Self {
        Declaring::Refused(error)
    }
}

impl From<StorageError> for Declaring {
    fn from(error: StorageError) -> Self {
        match super::held::unanswered(&error) {
            true => Declaring::Unanswered(error),
            false => Declaring::Refused(error.into()),
        }
    }
}
