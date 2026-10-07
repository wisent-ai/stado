//! Run AutoVersion's shared fixtures against this crate's port of the rule.
//!
//! `FIXTURES.md` holds the cases in its first fenced block: every `classify`
//! case must yield exactly the recorded class, next version, removed and
//! added names; every `refuse` case exactly the recorded refusal. A port that
//! misses one is not a port of the rule, and its verdicts are not trusted.
//! `pinned` reads the fixtures at the one tag this port follows, so a caller
//! (a product's workflow, `app-check`) never spells the coordinate itself.

use std::process::Command;

use serde_json::Value;

use super::rule::decide;
use crate::cli::CmdError;

const FIXTURES_URL: &str =
    "https://raw.githubusercontent.com/lbartoszcze/AutoVersion/v0.1.0/FIXTURES.md";

/// AutoVersion's FIXTURES.md at the tag this port follows. Each failure
/// states its class: no `curl` to start is this machine's environment
/// (`config`); a failed fetch or bytes that are not text are the network or
/// the remote file (`infra_down`), which a later run can get past.
pub(super) fn pinned() -> Result<String, CmdError> {
    let output = Command::new("curl")
        .args(["-fsSL", FIXTURES_URL])
        .output()
        .map_err(|error| CmdError::declaration(format!("curl could not start: {error}")))?;
    if !output.status.success() {
        return Err(CmdError::unreachable(format!(
            "{FIXTURES_URL} could not be read: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| CmdError::unreachable(format!("{FIXTURES_URL}: not UTF-8 ({error})")))
}

fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The JSON between the first pair of ``` fences, or the whole text when it
/// is already JSON.
fn cases(text: &str) -> Result<Value, String> {
    if let Ok(value) = serde_json::from_str(text) {
        return Ok(value);
    }
    let mut blocks = text.split("```");
    let block = blocks
        .nth(1)
        .ok_or("the fixtures file has no fenced block")?;
    let body = block.split_once('\n').map_or(block, |(_, rest)| rest);
    serde_json::from_str(body).map_err(|error| format!("the fenced fixtures are not JSON: {error}"))
}

fn outcome(case: &Value) -> Value {
    match decide(
        case["current"].as_str().unwrap_or(""),
        &names(&case["published"]),
        &names(&case["candidate"]),
        case["declared_breaking"].as_bool().unwrap_or(false),
    ) {
        Ok(answer) => serde_json::json!({
            "class": answer.change.name(),
            "next": answer.next,
            "removed": answer.removed,
            "added": answer.added,
        }),
        Err(refusal) => serde_json::json!({ "refusal": refusal.name }),
    }
}

/// Print one line per case and the count; `Ok(false)` when any case differs.
pub(super) fn run(text: &str) -> Result<bool, String> {
    let fixtures = cases(text)?;
    let mut failures = 0;
    let mut total = 0;
    for group in ["classify", "refuse"] {
        for case in fixtures[group]
            .as_array()
            .ok_or(format!("the fixtures declare no `{group}` list"))?
        {
            total += 1;
            let observed = outcome(case);
            let name = case["name"].as_str().unwrap_or("unnamed case");
            if observed == case["expect"] {
                println!("OK   {name}");
            } else {
                failures += 1;
                println!(
                    "FAIL {name}: expected {}, observed {observed}",
                    case["expect"]
                );
            }
        }
    }
    println!("{} of {total} fixture case(s) reproduced", total - failures);
    Ok(failures == 0)
}
