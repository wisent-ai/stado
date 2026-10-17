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
    let output = crate::wait::output(Command::new("curl").args(["-fsSL", FIXTURES_URL]))
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

/// A surface the case states as an array of names. A field missing or not an
/// array is a malformed case, named as such: read as an empty surface it
/// reproduced the `empty-surface` refusal by accident.
fn names(case: &Value, field: &str) -> Result<Vec<String>, String> {
    let items = case[field]
        .as_array()
        .ok_or_else(|| format!("the case states no {field} array"))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{field} holds {item}, which is not a name"))
        })
        .collect()
}

/// The two groups of AutoVersion's fixtures document; a document missing
/// either is refused by serde naming it.
#[derive(serde::Deserialize)]
struct Fixtures {
    classify: Vec<Value>,
    refuse: Vec<Value>,
}

/// The fixtures document: the whole text when it is JSON, otherwise the
/// first fenced block below its fence line.
fn cases(text: &str) -> Result<Fixtures, String> {
    // A text that is JSON is the document itself, and what it lacks is said;
    // a text that is not JSON carries the document in a fence.
    if let Ok(document) = serde_json::from_str::<Value>(text) {
        return serde_json::from_value(document)
            .map_err(|error| format!("the fixtures are not valid: {error}"));
    }
    let (_, opened) = text
        .split_once("```")
        .ok_or("the fixtures file is not JSON and has no fenced block")?;
    let (block, _) = opened
        .split_once("```")
        .ok_or("the fixtures file's fenced block is never closed")?;
    let (_, body) = block
        .split_once('\n')
        .ok_or("the fixtures file's fenced block holds nothing below its fence line")?;
    serde_json::from_str(body)
        .map_err(|error| format!("the fenced fixtures are not valid: {error}"))
}

/// The rule's answer to one case, or what makes the case malformed. A case
/// without `current` was decided on "" and one without a surface on an empty
/// one, so a broken fixture could pass as a refusal it never tested.
fn outcome(case: &Value) -> Result<Value, String> {
    let current = case["current"]
        .as_str()
        .ok_or("the case states no current version")?;
    // The refuse cases leave the declaration out: an absent declaration is
    // an unset one, as AutoVersion's fixtures write it; any other value that
    // is not a boolean is malformed.
    let declared_breaking = match case.get("declared_breaking") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("declared_breaking is {value}, not a boolean"))?,
    };
    Ok(
        match decide(
            current,
            &names(case, "published")?,
            &names(case, "candidate")?,
            declared_breaking,
        ) {
            Ok(answer) => serde_json::json!({
                "class": answer.change.name(),
                "next": answer.next,
                "removed": answer.removed,
                "added": answer.added,
            }),
            Err(refusal) => serde_json::json!({ "refusal": refusal.name }),
        },
    )
}

/// Whether one case reproduces: what differs, or what makes it malformed.
fn check(case: &Value) -> Result<(), String> {
    let observed = outcome(case).map_err(|malformed| format!("malformed case: {malformed}"))?;
    if observed == case["expect"] {
        Ok(())
    } else {
        Err(format!("expected {}, observed {observed}", case["expect"]))
    }
}

/// Print one line per case and the count; `Ok(false)` when any case differs
/// or is malformed. A case without a name is printed whole, not as
/// "unnamed case".
pub(super) fn run(text: &str) -> Result<bool, String> {
    let fixtures = cases(text)?;
    let all: Vec<&Value> = fixtures.classify.iter().chain(&fixtures.refuse).collect();
    let mut failed = Vec::new();
    for case in &all {
        let name = match case["name"].as_str() {
            Some(name) => name.to_string(),
            None => format!("(a case that states no name: {case})"),
        };
        match check(case) {
            Ok(()) => println!("OK   {name}"),
            Err(why) => {
                println!("FAIL {name}: {why}");
                failed.push(name);
            }
        }
    }
    println!(
        "{} of {} fixture case(s) reproduced",
        all.len() - failed.len(),
        all.len()
    );
    Ok(failed.is_empty())
}
