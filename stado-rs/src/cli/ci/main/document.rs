//! The main check's record as `stado ci status` and `stado ci delivery`
//! carry it, and the sentences the delivery path is read from: each step
//! whose main is red, since when, and each register write still owed.

use serde_json::{json, Value};

use super::record;
use crate::cli::reporting::field;
use crate::cli::CmdError;

/// The record as `stado ci status` and `stado ci delivery` carry it: per
/// step its last checked and last green commit, and every commit checked.
pub(in crate::cli::ci) fn document() -> Result<Value, CmdError> {
    let checked = record::read()?;
    let steps: serde_json::Map<String, Value> = checked
        .steps
        .iter()
        .map(|(key, commits)| {
            (
                key.clone(),
                json!({
                    "last_checked": checked.last_checked(key),
                    "last_green": checked.last_green(key),
                    "checked": commits,
                }),
            )
        })
        .collect();
    Ok(json!({ "record": record::path()?, "steps": steps }))
}

/// When `step`'s main turned red: the check of the first red commit after
/// its last green one, every commit checked being kept in the order it was
/// checked.
fn red_since(step: &Value) -> String {
    let checked = step["checked"].as_array().into_iter().flatten();
    let reds: Vec<&Value> = checked
        .rev()
        .take_while(|commit| commit["green"] == false)
        .collect();
    match reds.last() {
        Some(first) => format!("since {}", field(&first["at"])),
        None => "with no red commit in its record".to_string(),
    }
}

/// Each step whose last checked commit is red, as one sentence naming since
/// when it stands red.
pub(in crate::cli::ci) fn red(document: &Value) -> Vec<String> {
    document["steps"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, step)| step["last_checked"]["green"] == false)
        .map(|(key, step)| {
            let last = &step["last_checked"];
            format!(
                "main of {key} is red {} at {} (installs stay at {}): {}",
                red_since(step),
                field(&last["revision"]),
                field(&step["last_green"]["revision"]),
                field(&last["refusals"])
            )
        })
        .collect()
}

/// Each write the register refused about a step's last checked commit, or
/// about a commit whose repair records are still to be recorded again, as
/// one sentence naming the step and the commit.
pub(in crate::cli::ci) fn unrecorded(document: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    for (key, step) in document["steps"].as_object().into_iter().flatten() {
        let last = &step["last_checked"];
        let commits = step["checked"].as_array().into_iter().flatten();
        let open = commits.filter(|commit| {
            let unrepaired = commit["unrepaired"].as_array();
            unrepaired.is_some_and(|defects| !defects.is_empty())
                || commit["revision"] == last["revision"]
        });
        for commit in open {
            let refused = commit["unrecorded"].as_array().into_iter().flatten();
            for line in refused.filter_map(Value::as_str) {
                lines.push(format!("main of {key} at {}: {line}", field(&commit["revision"])));
            }
        }
    }
    lines
}

/// The record's lines in `stado ci status`'s and `stado ci delivery`'s plain
/// form.
pub(in crate::cli::ci) fn print(document: &Value) {
    for (key, step) in document["steps"].as_object().into_iter().flatten() {
        let last = &step["last_checked"];
        let verdict = match last["green"].as_bool() {
            Some(true) => "green",
            Some(false) => "red",
            None => "not checked",
        };
        println!(
            "  main of {key}: last checked {} {verdict}; last green {}",
            field(&last["revision"]), field(&step["last_green"]["revision"])
        );
        for commit in step["checked"].as_array().into_iter().flatten() {
            let unrepaired = commit["unrepaired"].as_array().into_iter().flatten();
            for defect in unrepaired.filter_map(Value::as_str) {
                let revision = field(&commit["revision"]);
                println!("    {revision}'s repair of {defect} is still to be recorded in Oko");
            }
        }
    }
}
