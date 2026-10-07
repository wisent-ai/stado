//! `stado capacity hold`: take a declared workload kind's reservation on a
//! host and keep it heartbeated until the caller signals the end, then
//! release it. The hold ends on SIGINT, SIGTERM or SIGHUP, never on a clock.

use serde_json::json;

use crate::cli::registry::read_registry;
use crate::cli::CmdError;
use crate::fleet_needs::this_requester;

use super::reserve::reserve_for_workload;

pub(super) async fn hold(kind: &str, target: &str, json_output: bool) -> Result<(), CmdError> {
    let declaration = crate::cli::workload::declared(kind)?;
    let registry = read_registry().await?;
    let target = registry
        .targets
        .iter()
        .find(|candidate| candidate.name == target)
        .ok_or_else(|| {
            CmdError::click(format!(
                "target '{target}' is not declared; add it to the canonical registry"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let holder = format!("{} hold pid {}", this_requester(), std::process::id());
    let held = match reserve_for_workload(declaration, target, holder).await? {
        Ok(held) => held,
        Err(refusal) => return Err(refusal.into_error()),
    };
    let reservation = held
        .reservation()
        .cloned()
        .ok_or_else(|| CmdError::refused("the hold was released before it began"))?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "held": reservation,
                "key": reservation.key(),
            }))?
        );
    } else {
        println!(
            "holding {} on {} as {} until interrupted ({} cores, {} GiB, {} GiB VRAM)",
            reservation.kind,
            reservation.target,
            reservation.reservation_id,
            reservation.cpu_cores,
            reservation.ram_gb,
            reservation.vram_gb
        );
    }
    until_released().await?;
    held.release().await.map_err(|error| {
        let message = format!("the hold could not be released: {error}");
        let mut failed = CmdError::from(error);
        failed.message = Some(message);
        failed
    })?;
    if !json_output {
        println!("released {}", reservation.reservation_id);
    }
    Ok(())
}

/// Returns when the process receives SIGINT, SIGTERM or SIGHUP.
#[cfg(unix)]
async fn until_released() -> Result<(), CmdError> {
    use tokio::signal::unix::{signal, SignalKind};
    let listen = |kind: SignalKind| {
        signal(kind).map_err(|error| {
            CmdError::click(format!("could not listen for the release signal: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })
    };
    let mut interrupt = listen(SignalKind::interrupt())?;
    let mut terminate = listen(SignalKind::terminate())?;
    let mut hangup = listen(SignalKind::hangup())?;
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
        _ = hangup.recv() => {}
    }
    Ok(())
}

/// Returns when the process receives Ctrl-C.
#[cfg(not(unix))]
async fn until_released() -> Result<(), CmdError> {
    tokio::signal::ctrl_c().await.map_err(|error| {
        CmdError::click(format!("could not listen for the release signal: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })
}
