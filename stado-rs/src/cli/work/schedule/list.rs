//! `stado schedule list`: the fleet store's schedules and the declarations
//! held on this host, sorted by next run (paused last).

use serde_json::Value;

use crate::queue::{JobStorage, StorageError};
use crate::schedules::{self, Schedule};

use crate::cli::CmdError;

/// The store's schedules after the held declarations were written into it,
/// with the coordinator's last recorded schedule sweep they are judged
/// overdue against, or the store's error.
async fn stored() -> Result<Result<(Vec<Schedule>, Option<schedules::Sweep>), CmdError>, StorageError> {
    let store = JobStorage::new().await?;
    if let Err(error) = super::held::deliver(&store).await? {
        return Ok(Err(error));
    }
    let listed = schedules::list_schedules(&store).await?;
    Ok(Ok((listed, schedules::last_sweep(&store).await?)))
}

/// `schedule list`: every schedule the fleet store holds and every
/// declaration held on this host, each held one marked with its file and
/// the store's refusal. A schedule still waiting past its next run carries
/// `overdue`, why it has not fired, read against the coordinator's last
/// recorded sweep as `schedule show` reads it, so one listing shows every
/// routine that stopped firing. A store that does not answer still lists
/// what is held here, says why the store's own were not listed, and exits
/// with that refusal.
pub async fn list(json: bool) -> Result<(), CmdError> {
    let (mut scheds, sweep, unanswered) = match stored().await {
        Ok(listed) => {
            let (scheds, sweep) = listed?;
            (scheds, sweep, None)
        }
        Err(error) if super::held::unanswered(&error) => (Vec::new(), None, Some(error)),
        Err(error) => return Err(error.into()),
    };
    let held = super::held::all()?;
    // Python sort key: s.next_due_at or "~" — plain string sort, paused
    // ("" → "~") last.
    let order = |s: &Schedule| match s.next_due_at.is_empty() {
        true => "~".to_string(),
        false => s.next_due_at.clone(),
    };
    scheds.sort_by_key(order);
    let now = chrono::Utc::now();
    if json {
        let mut rows: Vec<Value> = Vec::new();
        for s in &scheds {
            let mut row: Value = serde_json::from_str(&s.to_json())?;
            if let Some(reason) = schedules::overdue(s, sweep.as_ref(), now) {
                row["overdue"] = Value::String(reason);
            }
            rows.push(row);
        }
        for (path, declaration) in &held {
            let mut row: Value = serde_json::from_str(&declaration.schedule.to_json())?;
            row["held"] = serde_json::json!({
                "file": path.display().to_string(),
                "refusal": declaration.refusal,
                "held_at": declaration.held_at,
            });
            rows.push(row);
        }
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print_table(&scheds, &held, |s| schedules::overdue(s, sweep.as_ref(), now));
    }
    match unanswered {
        None => Ok(()),
        Some(error) => Err(CmdError::refused(format!(
            "the fleet store did not answer ({error}), so only the schedules held on this host \
             are listed"
        ))),
    }
}

/// The table of `scheds` and `held`; a schedule `overdue` names a reason for
/// is followed by a line saying why it has not fired.
fn print_table(
    scheds: &[Schedule],
    held: &[(std::path::PathBuf, super::held::Held)],
    overdue: impl Fn(&Schedule) -> Option<String>,
) {
    if scheds.is_empty() && held.is_empty() {
        println!("(no schedules)");
        return;
    }
    let header = format!(
        "{:<14} {:<3} {:<16} {:<14} {:<28} {:>5} COMMAND",
        "ID", "EN", "CRON", "TZ", "NEXT RUN (UTC)", "FIRED"
    );
    println!("{header}");
    println!("{}", "-".repeat(header.len()));
    for s in scheds {
        let en = if s.enabled { "Y" } else { "n" };
        println!("{}", row(s, en));
        if let Some(reason) = overdue(s) {
            println!("  overdue: {reason}");
        }
    }
    for (path, declaration) in held {
        println!("{}", row(&declaration.schedule, "H"));
        println!(
            "  held on this host at {} since {}: {}",
            path.display(),
            declaration.held_at,
            declaration.refusal
        );
    }
    let count = scheds.len();
    if held.is_empty() {
        println!("\n{count} schedule(s)");
    } else {
        println!("\n{count} schedule(s), {} held on this host", held.len());
    }
}

/// One table row; the columns pad short values and the command runs last.
fn row(s: &Schedule, en: &str) -> String {
    let cmd = s.command.split_whitespace().collect::<Vec<_>>().join(" ");
    let next = match s.next_due_at.is_empty() {
        true => "-",
        false => s.next_due_at.as_str(),
    };
    format!(
        "{:<14} {:<3} {:<16} {:<14} {:<28} {:>5} {}",
        s.schedule_id, en, s.cron, s.tz, next, s.fire_count, cmd
    )
}
