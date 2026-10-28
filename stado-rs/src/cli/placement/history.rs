//! `stado placement history [--json]`: every placement change this machine
//! ran, newest first — profile moves, role moves and authority moves, with
//! their receipts or the refusal that stopped them.
//!
//! A move's journal is swept once the move ends, and its receipt was printed
//! once and gone, so after an evacuation nothing on the machine said what
//! moved where, when, or which step stopped it. Each change is kept here as
//! one document (`~/.stado/placement-history/<time>-<id>.json`, the
//! directory `STADO_PLACEMENT_HISTORY_DIR` names), written whole and durably.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::cli::CmdError;

fn directory() -> Result<PathBuf, String> {
    crate::primitives::document::located(
        "STADO_PLACEMENT_HISTORY_DIR",
        "placement-history",
        "placement history",
    )
}

/// Keep one placement change: `kind` (`move`, `move-role`, `authority`),
/// and its receipt or the refusal that stopped it. The change already
/// happened or failed, so a record that cannot be kept is said on stderr and
/// does not turn the command's answer into a failure.
pub(crate) fn record(kind: &str, outcome: Result<&Value, &CmdError>) {
    let recorded_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let entry = match outcome {
        Ok(receipt) => json!({"recorded_at": recorded_at, "kind": kind, "receipt": receipt}),
        Err(error) => {
            json!({"recorded_at": recorded_at, "kind": kind, "refused": error.to_string()})
        }
    };
    let kept = directory().and_then(|directory| {
        let name = format!("{recorded_at}-{}.json", uuid::Uuid::new_v4());
        crate::primitives::document::write(&directory.join(name), &entry, "placement history entry")
    });
    if let Err(cause) = kept {
        eprintln!("stado: the placement {kind} is not kept in the placement history: {cause}");
    }
}

/// Every kept change, newest first; an entry that cannot be read is named
/// in the answer, not skipped.
fn entries() -> Result<(Vec<Value>, Vec<String>), CmdError> {
    let directory = directory().map_err(CmdError::declaration)?;
    let listing = match std::fs::read_dir(&directory) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()))
        }
        Err(error) => {
            return Err(CmdError::click(format!(
                "the placement history {} cannot be listed: {error}",
                directory.display()
            )))
        }
    };
    let mut kept = Vec::new();
    let mut unread = Vec::new();
    for entry in listing {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                unread.push(format!("{}: {error}", directory.display()));
                continue;
            }
        };
        match crate::primitives::document::read::<Value>(&path, "placement history entry") {
            Ok(Some(value)) => kept.push(value),
            Ok(None) => {}
            Err(cause) => unread.push(cause),
        }
    }
    kept.sort_by(|left, right| {
        right["recorded_at"]
            .as_str()
            .cmp(&left["recorded_at"].as_str())
    });
    Ok((kept, unread))
}

pub(super) fn list(json_output: bool) -> Result<(), CmdError> {
    let (kept, unread) = entries()?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"entries": kept, "unread": unread}))?
        );
        return Ok(());
    }
    if kept.is_empty() {
        println!("no placement change is kept on this machine");
    }
    for entry in &kept {
        use crate::cli::reporting::field;
        let receipt = &entry["receipt"];
        // An authority move names its hosts `fenced` and `authority`.
        let host = |named: &str, authority_named: &str| match &receipt[named] {
            Value::Null => field(&receipt[authority_named]),
            present => field(present),
        };
        let from = host("from_host", "fenced");
        let to = host("to_host", "authority");
        let what = match receipt
            .get("profile")
            .or_else(|| receipt.get("service"))
            .and_then(Value::as_str)
        {
            Some(name) => format!(" {name}"),
            None => String::new(),
        };
        match entry.get("refused").and_then(Value::as_str) {
            Some(refused) => println!(
                "{} {} refused: {refused}",
                field(&entry["recorded_at"]),
                field(&entry["kind"])
            ),
            None => println!(
                "{} {}{what}: {from} -> {to} ({})",
                field(&entry["recorded_at"]),
                field(&entry["kind"]),
                field(&receipt["state"])
            ),
        }
    }
    for cause in &unread {
        eprintln!("stado: a placement history entry cannot be read: {cause}");
    }
    Ok(())
}
