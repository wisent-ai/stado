//! `stado host console show|record|unset TARGET [--json]` — whether anybody
//! can answer a question macOS shows at a Mac's screen, measured on the host
//! and recorded as `targets[].console`.
//!
//! The recorded console decides where Stado's units run on a Mac
//! ([`crate::deploy::service::requires_daemon_domain`]) and which dialogs
//! `stado host upgrade` may plan there, so it is never typed: `record`
//! writes the measurement, `show` confronts the record with a fresh one.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::host_console::{self, Measured};
use crate::primitives::failure::FailureCode;
use crate::targets::{ConsoleDeclaration, ConsoleState, CONSOLE_KEY};

/// What the record says beside the measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Agrees,
    Differs,
    /// Nothing is recorded, and the host measures attended: Stado treats a
    /// host with no record as attended, so nothing is wrong.
    Unrecorded,
    /// Nothing is recorded, and the host measures headless.
    Missing,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Self::Agrees => "agrees",
            Self::Differs => "differs",
            Self::Unrecorded => "unrecorded",
            Self::Missing => "missing",
        }
    }
}

fn measured_json(measured: &Measured) -> Value {
    json!({
        "console_owner": measured.owner,
        "display_panels": measured.displays,
        "displays_reported": measured.reported,
        "declaration": measured.declaration,
        "evidence": measured.declaration.evidence(),
    })
}

/// Measure `target` through its declared connection.
async fn measure(target: &crate::targets::ComputeTarget) -> Result<Measured, CmdError> {
    host_console::measure(target, &crate::deploy::production_runner())
        .await
        .map_err(CmdError::from)
}

/// `stado host console show TARGET`: the recorded console beside a fresh
/// measurement. Exits non-zero when the record differs from the host, or
/// when nothing is recorded for a host that measures headless.
pub async fn show(target: &str, json: bool) -> Result<(), CmdError> {
    let resolved = crate::cli::canonical_host(target).await?;
    let recorded = resolved
        .console()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    let measured = measure(&resolved).await?;
    let name = &resolved.name;
    let fix = format!("`stado host console record {name}` records the measurement");
    let (verdict, refusal) = match &recorded {
        Some(recorded) if host_console::agrees(recorded, &measured.declaration) => {
            (Verdict::Agrees, None)
        }
        Some(recorded) => (
            Verdict::Differs,
            Some(format!(
                "{name} records a {} console ({}), and it measures {} now ({}); {fix}",
                recorded.state,
                recorded.evidence(),
                measured.declaration.state,
                measured.declaration.evidence()
            )),
        ),
        None if measured.declaration.state == ConsoleState::Headless => (
            Verdict::Missing,
            Some(format!(
                "{name} measures headless ({}) and records no console, so Stado runs its units \
                 there as LaunchAgents, which macOS asks questions nobody answers; {fix}",
                measured.declaration.evidence()
            )),
        ),
        None => (Verdict::Unrecorded, None),
    };
    if json {
        crate::cli::host::checks::probes::print_json(&json!({
            "target": name,
            "measured": measured_json(&measured),
            "recorded": recorded,
            "verdict": verdict.word(),
            "error": refusal,
        }));
        return match refusal {
            Some(_) => Err(CmdError::silent(CmdError::refused("").code)),
            None => Ok(()),
        };
    }
    println!(
        "measured: {} — {} (/dev/console belongs to {}{})",
        measured.declaration.state,
        measured.declaration.evidence(),
        measured.owner,
        if measured.displays.is_empty() {
            String::new()
        } else {
            format!("; panels: {}", measured.displays.join(", "))
        }
    );
    match &recorded {
        Some(recorded) => println!(
            "recorded: {} — {} (measured {})",
            recorded.state,
            recorded.evidence(),
            recorded.measured_at
        ),
        None => println!("recorded: nothing"),
    }
    println!("{name}: {}", verdict.word());
    match refusal {
        Some(refusal) => Err(CmdError::refused(refusal)),
        None => Ok(()),
    }
}

/// Write `console` into `target`'s registry entry, or remove the key for
/// `None`, validated as the whole document is. Returns the new generation.
pub(crate) async fn write(
    target: &str,
    console: Option<&ConsoleDeclaration>,
) -> Result<String, CmdError> {
    let store = crate::targets::RegistryStore::open().await?;
    let current = store.read_versioned().await?.ok_or_else(|| {
        CmdError::click("canonical registry generation unavailable").stating(FailureCode::InfraDown)
    })?;
    let mut document: Value = serde_json::from_str(&current.content)?;
    let entry = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            CmdError::click("registry.targets: must be an array").stating(FailureCode::Config)
        })?
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click(format!("target not in registry: {target}"))
                .stating(FailureCode::NotFound)
        })?;
    match console {
        Some(console) => {
            entry.insert(CONSOLE_KEY.to_string(), serde_json::to_value(console)?);
        }
        None => {
            entry.remove(CONSOLE_KEY);
        }
    }
    crate::targets::strip_retired_resource_declarations(&mut document);
    crate::targets::validate_registry(&document).map_err(CmdError::from)?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    Ok(store.compare_and_swap(&current.version, &payload).await?)
}

/// `stado host console record TARGET`: measure the host and record the
/// measurement.
pub async fn record(target: &str, json: bool) -> Result<(), CmdError> {
    let resolved = crate::cli::canonical_host(target).await?;
    let measured = measure(&resolved).await?;
    let generation = write(&resolved.name, Some(&measured.declaration)).await?;
    if json {
        crate::cli::host::checks::probes::print_json(&json!({
            "target": resolved.name,
            "measured": measured_json(&measured),
            "registry_generation": generation,
        }));
    } else {
        println!(
            "{}: recorded {} — {} (generation {generation})",
            resolved.name,
            measured.declaration.state,
            measured.declaration.evidence()
        );
    }
    Ok(())
}

/// `stado host console unset TARGET`: remove the recorded console; Stado
/// then treats the host as attended.
pub async fn unset(target: &str, json: bool) -> Result<(), CmdError> {
    let resolved = crate::cli::canonical_host(target).await?;
    let recorded = resolved
        .console()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    let generation = match recorded {
        Some(_) => Some(write(&resolved.name, None).await?),
        None => None,
    };
    if json {
        crate::cli::host::checks::probes::print_json(&json!({
            "target": resolved.name,
            "removed": generation.is_some(),
            "registry_generation": generation,
        }));
    } else {
        match generation {
            Some(generation) => println!(
                "{}: the recorded console was removed (generation {generation})",
                resolved.name
            ),
            None => println!("{}: records no console; nothing was changed", resolved.name),
        }
    }
    Ok(())
}
