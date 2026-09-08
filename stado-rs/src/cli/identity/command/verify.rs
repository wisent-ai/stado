//! `identity verify`: what each host confirms right now, and the exit code for it.

use anyhow::Result;
use serde_json::json;

use super::{verified_bindings, Verification};
use crate::cli::CmdError;
use crate::targets::load_registry_auto;

/// Resolve which host currently holds an identity, checking rather than trusting.
///
/// Exits non-zero when nothing satisfies the binding. That is the point: a caller
/// that needs a trusted device can gate on this and fail with "no host holds
/// <identity>" instead of dispatching work that cannot possibly complete.
pub async fn verify(kind: String, identity: String, json_output: bool) -> Result<(), CmdError> {
    let registry = load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let Verification { rows, satisfied } = verified_bindings(&registry, &kind, &identity).await;

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": kind,
                "identity": identity,
                "satisfied": satisfied,
                "bindings": rows,
            }))
            .unwrap_or_default()
        );
    } else if rows.is_empty() {
        println!("no host declares {kind} {identity}");
    } else {
        for row in &rows {
            let observed = match row["observed"].as_bool() {
                Some(true) => "held",
                Some(false) => "MISSING",
                None => "unknown",
            };
            // Two separate questions, so two separate words: whether the identity is
            // there, and whether the fleet can act where it is. False covers either a
            // different session or an incomplete GUI runtime; both refuse placement.
            let session = match row["drivable_session"].as_bool() {
                Some(true) => "drivable",
                Some(false) => "NOT-DRIVABLE",
                None => "unknown",
            };
            println!(
                "{:<24} {:<32} {:<8} {}",
                row["host"].as_str().unwrap_or("-"),
                row["identity"].as_str().unwrap_or("-"),
                observed,
                session
            );
        }
    }

    if satisfied {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "no host holds {kind} {identity}; enroll one before dispatching work that needs it"
        )))
    }
}
