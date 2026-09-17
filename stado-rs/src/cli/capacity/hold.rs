//! `stado capacity hold`: take a declared workload kind's reservation on a
//! host for a fixed time and keep it heartbeated, then release it.

use std::time::Duration;

use serde_json::json;

use crate::cli::registry::read_registry;
use crate::cli::CmdError;
use crate::fleet_needs::this_requester;

use super::reserve::reserve_for_workload;

pub(super) async fn hold(
    kind: &str,
    target: &str,
    seconds: u64,
    json_output: bool,
) -> Result<(), CmdError> {
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
        })?;
    let holder = format!(
        "{} hold {}s pid {}",
        this_requester(),
        seconds,
        std::process::id()
    );
    let held = match reserve_for_workload(declaration, target, holder).await? {
        Ok(held) => held,
        Err(refusal) => return Err(refusal.into_error()),
    };
    let reservation = held
        .reservation()
        .cloned()
        .ok_or_else(|| CmdError::click("the hold was released before it began"))?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "held": reservation,
                "seconds": seconds,
                "key": reservation.key(),
            }))?
        );
    } else {
        println!(
            "holding {} on {} as {} for {seconds}s ({} cores, {} GiB, {} GiB VRAM)",
            reservation.kind,
            reservation.target,
            reservation.reservation_id,
            reservation.cpu_cores,
            reservation.ram_gb,
            reservation.vram_gb
        );
    }
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    held.release()
        .await
        .map_err(|error| CmdError::click(format!("the hold could not be released: {error}")))?;
    if !json_output {
        println!("released {}", reservation.reservation_id);
    }
    Ok(())
}
