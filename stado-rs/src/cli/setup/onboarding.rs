//! First-use walkthrough for the Stado CLI.
//!
//! The copy lives only in `onboarding_first_use.json`. This module presents
//! that definition and keeps device-local progress beside Stado's other state.

mod render;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{json, Value};

use self::render::render;
use crate::cli::CmdError;

const PRODUCT_ID: &str = "stado-cli";
const JOURNEY_ID: &str = "first-use";
const FIRST_SUCCESS_FACT: &str = "registry_configuration_accepted";
const DEFINITION: &str = include_str!("../../onboarding_first_use.json");
const COMPLETED_MESSAGE: &str = "First-run journey already complete: an existing registry configuration was accepted by the canonical store. Use `stado onboarding --reset` to show it again, or `stado registry import PATH` to adopt another non-conflicting registry.";

/// The shipped journey, refused when it names another product, journey or
/// first-success fact than this module presents. It is built into the binary,
/// so its shape is the one this module reads, and every screen field it lacks
/// is said on the screen itself.
fn definition() -> Result<Value, CmdError> {
    let definition: Value = serde_json::from_str(DEFINITION)?;
    if definition.get("product_id").and_then(Value::as_str) != Some(PRODUCT_ID)
        || definition.get("journey_id").and_then(Value::as_str) != Some(JOURNEY_ID)
        || definition.get("first_success_fact").and_then(Value::as_str) != Some(FIRST_SUCCESS_FACT)
    {
        return Err(
            CmdError::click("shipped onboarding journey has an invalid identity")
                .stating(crate::primitives::failure::FailureCode::Config),
        );
    }
    Ok(definition)
}

/// `~/.stado/onboarding.json` under the home every Stado file is kept under
/// ([`crate::config_file::home`]: HOME, else the user database).
fn state_path() -> Result<PathBuf, CmdError> {
    let home = crate::config_file::home().map_err(|cause| {
        CmdError::click(format!(
            "onboarding progress has nowhere to be kept: {cause}"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    Ok(home.join(".stado/onboarding.json"))
}

fn read_state(path: &Path) -> Result<Option<Value>, CmdError> {
    match fs::read_to_string(path) {
        Ok(body) => Ok(Some(serde_json::from_str(&body).map_err(|error| {
            CmdError::click(format!(
                "cannot parse onboarding state {}: {error}; use `stado onboarding --reset` to replace it",
                path.display()
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CmdError::click(format!(
            "cannot read onboarding state {}: {error}",
            path.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))),
    }
}

/// Keep `state` whole through [`crate::primitives::document::persist`].
fn write_state(path: &Path, state: &Value) -> Result<(), CmdError> {
    let body = format!("{}\n", serde_json::to_string_pretty(state)?);
    crate::primitives::document::persist(path, body.as_bytes())?;
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
        .ok_or_else(|| {
            CmdError::click("shipped onboarding journey has no screens")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let mut next_id = definition
        .get("entry_screen_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::click("shipped onboarding journey has no entry screen")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();

    loop {
        if !seen.insert(next_id) {
            return Err(
                CmdError::click("shipped onboarding journey contains a transition cycle")
                    .stating(crate::primitives::failure::FailureCode::Config),
            );
        }
        let screen = screens
            .iter()
            .find(|screen| screen.get("screen_id").and_then(Value::as_str) == Some(next_id))
            .ok_or_else(|| {
                CmdError::click(format!(
                    "shipped onboarding journey has no screen `{next_id}`"
                ))
                .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        let presentation = screen
            .get("presentation")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                CmdError::click(format!("onboarding screen `{next_id}` has no presentation"))
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        if presentation.get("title").and_then(Value::as_str).is_none()
            || presentation.get("body").and_then(Value::as_str).is_none()
        {
            return Err(CmdError::click(format!(
                "onboarding screen `{next_id}` has incomplete presentation copy"
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }
        ordered.push(screen);

        // The lowest priority wins, the first of equals as before. A
        // transition without one was ranked as priority 0 and could win over
        // every declared one, so which screen came next was decided by a
        // missing value.
        let mut chosen: Option<(i64, &Value)> = None;
        for transition in screen
            .get("transitions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(priority) = transition.get("priority").and_then(Value::as_i64) else {
                return Err(CmdError::click(format!(
                    "onboarding screen `{next_id}` has a transition without an integer priority: \
                     {transition}"
                ))
                .stating(crate::primitives::failure::FailureCode::Config));
            };
            if chosen.is_none_or(|(best, _)| priority < best) {
                chosen = Some((priority, transition));
            }
        }
        let Some((_, next)) = chosen else {
            break;
        };
        next_id = next
            .get("next_screen_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CmdError::click("onboarding transition has no target")
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
    }

    Ok(ordered)
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

    // No progress file is a journey not begun on this device, which is what
    // a fresh state records.
    let mut state = match existing {
        Some(state) => state,
        None => new_state(&definition),
    };
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
        return crate::cli::registry::import(source, json_output).await;
    }

    for (index, screen) in screens.iter().enumerate() {
        // The screens up to and including this one: its place counted from
        // the first, as the reader counts it.
        render(screen, screens[..=index].len(), screens.len());
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
    let mut state = match read_state(&path)? {
        Some(state) => state,
        None => new_state(&definition),
    };
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
            "state": receipt.state,
        },
    });
    state["completed_at"] = Value::String(Utc::now().to_rfc3339());
    write_state(&path, &state)
}
