//! `stado schedule list`: the fleet store's schedules and the declarations
//! held on this host, sorted by next run (paused last).

use serde_json::Value;

use crate::queue::{JobStorage, StorageError};
use crate::schedules::{self, Schedule};

use crate::cli::CmdError;

/// The store's schedules after the held declarations were written into it,
/// or the store's error.
async fn stored() -> Result<Result<Vec<Schedule>, CmdError>, StorageError> {
    let store = JobStorage::new().await?;
    if let Err(error) = super::held::deliver(&store).await? {
        return Ok(Err(error));
    }
    Ok(Ok(schedules::list_schedules(&store).await?))
}

/// `schedule list`: every schedule the fleet store holds and every
/// declaration held on this host, each held one marked with its file and
/// the store's refusal. A store that does not answer still lists what is
/// held here, says why the store's own were not listed, and exits with that
/// refusal.
pub async fn list(json: bool) -> Result<(), CmdError> {
    let (mut scheds, unanswered) = match stored().await {
        Ok(listed) => (listed?, None),
        Err(error) if super::held::unanswered(&error) => (Vec::new(), Some(error)),
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
    if json {
        let mut rows: Vec<Value> = scheds
            .iter()
            .map(|s| serde_json::from_str(&s.to_json()))
            .collect::<Result<_, _>>()?;
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
        print_table(&scheds, &held);
    }
    match unanswered {
        None => Ok(()),
        Some(error) => Err(CmdError::refused(format!(
            "the fleet store did not answer ({error}), so only the schedules held on this host \
             are listed"
        ))),
    }
}

fn print_table(scheds: &[Schedule], held: &[(std::path::PathBuf, super::held::Held)]) {
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
