//! First-use walkthrough for the Stado CLI.
//!
//! The copy lives only in `onboarding_first_use.json`. This module presents
//! that definition and keeps device-local progress beside Stado's other state.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{json, Value};

use super::CmdError;

const PRODUCT_ID: &str = "stado-cli";
const JOURNEY_ID: &str = "first-use";
const FIRST_SUCCESS_FACT: &str = "registry_configuration_accepted";
const DEFINITION: &str = include_str!("../onboarding_first_use.json");
const COMPLETED_MESSAGE: &str = "First-run journey already complete: an existing registry configuration was accepted by the canonical store. Use `stado onboarding --reset` to show it again, or `stado registry import PATH` to adopt another non-conflicting registry.";

fn definition() -> Result<Value, CmdError> {
    let definition: Value = serde_json::from_str(DEFINITION)?;
    if definition.get("schema_version").and_then(Value::as_u64) != Some(1)
        || definition.get("product_id").and_then(Value::as_str) != Some(PRODUCT_ID)
        || definition.get("journey_id").and_then(Value::as_str) != Some(JOURNEY_ID)
        || definition.get("first_success_fact").and_then(Value::as_str) != Some(FIRST_SUCCESS_FACT)
    {
        return Err(CmdError::click(
            "shipped onboarding journey has an invalid identity",
        ));
    }
    Ok(definition)
}

fn state_path() -> Result<PathBuf, CmdError> {
    let home = std::env::var_os("HOME").ok_or_else(|| CmdError::click("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".stado/onboarding.json"))
}

fn read_state(path: &Path) -> Result<Option<Value>, CmdError> {
    match fs::read_to_string(path) {
        Ok(body) => Ok(Some(serde_json::from_str(&body).map_err(|error| {
            CmdError::click(format!(
                "cannot parse onboarding state {}: {error}; use `stado onboarding --reset` to replace it",
                path.display()
            ))
        })?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CmdError::click(format!(
            "cannot read onboarding state {}: {error}",
            path.display()
        ))),
    }
}

fn write_state(path: &Path, state: &Value) -> Result<(), CmdError> {
    let parent = path
        .parent()
        .ok_or_else(|| CmdError::click("onboarding state path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(
        &temporary,
        format!("{}\n", serde_json::to_string_pretty(state)?),
    )?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn new_state(definition: &Value) -> Value {
    json!({
        "product_id": PRODUCT_ID,
        "journey_id": JOURNEY_ID,
        "journey_version": definition.get("journey_version"),
        "status": "in_progress",
        "evidence": {},
        "presented_screen_ids": [],
        "started_at": Utc::now().to_rfc3339(),
    })
}

fn ordered_screens(definition: &Value) -> Result<Vec<&Value>, CmdError> {
    let screens = definition
        .get("screens")
        .and_then(Value::as_array)
        .ok_or_else(|| CmdError::click("shipped onboarding journey has no screens"))?;
    let mut next_id = definition
        .get("entry_screen_id")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("shipped onboarding journey has no entry screen"))?;
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();

    loop {
        if !seen.insert(next_id) {
            return Err(CmdError::click(
                "shipped onboarding journey contains a transition cycle",
            ));
        }
        let screen = screens
            .iter()
            .find(|screen| screen.get("screen_id").and_then(Value::as_str) == Some(next_id))
            .ok_or_else(|| {
                CmdError::click(format!(
                    "shipped onboarding journey has no screen `{next_id}`"
                ))
            })?;
        let presentation = screen
            .get("presentation")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                CmdError::click(format!("onboarding screen `{next_id}` has no presentation"))
            })?;
        if presentation.get("title").and_then(Value::as_str).is_none()
            || presentation.get("body").and_then(Value::as_str).is_none()
        {
            return Err(CmdError::click(format!(
                "onboarding screen `{next_id}` has incomplete presentation copy"
            )));
        }
        ordered.push(screen);

        let Some(next) = screen
            .get("transitions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .min_by_key(|transition| {
                transition
                    .get("priority")
                    .and_then(Value::as_i64)
                    .unwrap_or_default()
            })
        else {
            break;
        };
        next_id = next
            .get("next_screen_id")
            .and_then(Value::as_str)
            .ok_or_else(|| CmdError::click("onboarding transition has no target"))?;
    }

    Ok(ordered)
}

fn render(screen: &Value, index: usize, total: usize) {
    let presentation = &screen["presentation"];
    println!(
        "{}/{}  {}\n       {}",
        index + 1,
        total,
        presentation["title"].as_str().unwrap_or_default(),
        presentation["body"].as_str().unwrap_or_default()
    );
    if let Some(command) = presentation.get("command").and_then(Value::as_str) {
        println!("       $ {command}");
    }
    if let Some(result) = presentation.get("result").and_then(Value::as_str) {
        println!("       Result: {result}");
    }
    println!();
}

pub async fn run(
    reset: bool,
    import_registry: Option<String>,
    json_output: bool,
) -> Result<(), CmdError> {
    let definition = definition()?;
    let screens = ordered_screens(&definition)?;
    let path = state_path()?;
    let existing = if reset { None } else { read_state(&path)? };

    if import_registry.is_none()
        && existing
            .as_ref()
            .and_then(|state| state.get("status"))
            .and_then(Value::as_str)
            == Some("completed")
    {
        println!("{COMPLETED_MESSAGE}");
        return Ok(());
    }

    let mut state = existing.unwrap_or_else(|| new_state(&definition));
    if reset {
        state = new_state(&definition);
        println!(
            "First-run walkthrough reset: recorded progress and evidence discarded, showing it again now.\n"
        );
    }

    let screen_ids = screens
        .iter()
        .filter_map(|screen| screen.get("screen_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    state["presented_screen_ids"] = json!(screen_ids);
    write_state(&path, &state)?;

    if let Some(source) = import_registry {
        return super::registry::import(source, json_output).await;
    }

    for (index, screen) in screens.iter().enumerate() {
        render(screen, index, screens.len());
    }
    println!(
        "First success is still open. Import an existing registry with the command on the final screen, or skip it and continue with an empty local registry."
    );
    Ok(())
}

/// Record the effect only after the canonical import operation has read back
/// the accepted generation. A rejected source or semantic conflict never
/// reaches this boundary.
pub fn record_registry_import_accepted(receipt: &crate::registry_import::RegistryImportReceipt) {
    if !receipt.accepted() {
        return;
    }
    if let Err(error) = try_record_registry_import_accepted(receipt) {
        eprintln!(
            "registry import was accepted, but first-run evidence was not recorded: {}",
            error
                .message
                .as_deref()
                .unwrap_or("onboarding state unavailable")
        );
    }
}

fn try_record_registry_import_accepted(
    receipt: &crate::registry_import::RegistryImportReceipt,
) -> Result<(), CmdError> {
    let definition = definition()?;
    let path = state_path()?;
    let mut state = read_state(&path)?.unwrap_or_else(|| new_state(&definition));
    if state.get("product_id").and_then(Value::as_str) != Some(PRODUCT_ID)
        || state.get("journey_id").and_then(Value::as_str) != Some(JOURNEY_ID)
    {
        return Ok(());
    }

    state["status"] = Value::String("completed".to_string());
    state["evidence"] = json!({
        (FIRST_SUCCESS_FACT): true,
        "registry_import": {
            "source_sha256": receipt.source_sha256.as_str(),
            "generation": receipt.generation.as_deref(),
            "state": receipt.state.as_str(),
        },
    });
    state["completed_at"] = Value::String(Utc::now().to_rfc3339());
    write_state(&path, &state)
}
