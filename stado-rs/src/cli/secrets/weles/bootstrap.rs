//! Recreating Weles's internal authorities in the canonical owner vault.

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::secrets::store::resolve::{launcher_json, owner_vault, skarbiec_binary};

fn generated_authority(
    binary: &std::path::Path,
    vault: &std::path::Path,
) -> Result<String, CmdError> {
    launcher_json(
        binary,
        vault,
        &[
            "generate", "--length", "64", "--lower", "--upper", "--digits",
        ],
    )?
    .get("password")
    .and_then(Value::as_str)
    .filter(|secret| !secret.is_empty())
    .map(str::to_string)
    .ok_or_else(|| CmdError::click("Skarbiec generator returned no authority value"))
}

/// Recreate Weles's internal authorities in the canonical owner vault.
///
/// These twelve items are authorities Weles issues to itself, so for a while
/// this command kept them in a vault of their own — it created
/// `weles-skarbiec.vault.json` under a `weles-skarbiec-owner` identity when the
/// path was missing. That made Weles the one writer in the fleet whose
/// credentials no other reader could open: `stado secrets ls`, the desktop
/// console and every consumer grant resolve against the canonical store, and an
/// item written into the side vault is absent from all of them.
///
/// The vault is now resolved, never created. A machine that holds no canonical
/// vault cannot recreate an authority, and saying so is the point: initializing
/// a fresh vault here would report twelve successful writes into a store that
/// nothing else on the host reads.
pub(crate) fn bootstrap_weles(json_output: bool) -> Result<(), CmdError> {
    let binary = skarbiec_binary()?;
    let vault = owner_vault()?;
    let database_role = crate::transcripts::value_for("WELES_SUPABASE_SERVICE_ROLE_KEY")
        .ok_or_else(|| {
            CmdError::click(
                "WELES_SUPABASE_SERVICE_ROLE_KEY is not recoverable from incident history",
            )
        })?;
    let operator_token =
        crate::transcripts::value_for("WELES_CONSOLE_API_TOKEN").ok_or_else(|| {
            CmdError::click("WELES_CONSOLE_API_TOKEN is not recoverable from incident history")
        })?;
    let model_router_token = generated_authority(&binary, &vault)?;
    let database_url = "https://rbqjqnouluslojmmnuqi.supabase.co";
    let agent_id = "weles";
    let items = vec![
        (
            "weles-database",
            json!({"url": database_url, "service_role_key": database_role}),
        ),
        (
            "weles-object-api",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        ("weles-model-router", json!({"token": model_router_token})),
        (
            "weles-model-agent-auth",
            json!({
                "id": agent_id,
                "agent_auth_secret": generated_authority(&binary, &vault)?,
            }),
        ),
        (
            "weles-artifact-delivery",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        (
            "weles-artifact-signing",
            json!({"signing_secret": generated_authority(&binary, &vault)?}),
        ),
        (
            "oko-weles-subscriptions",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        (
            "weles-content-diagnostics",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        (
            "weles-trading-tools-ingest",
            json!({
                "token": generated_authority(&binary, &vault)?,
                "hmac_secret": generated_authority(&binary, &vault)?,
            }),
        ),
        (
            "weles-operator-cdp",
            json!({
                "url": "http://127.0.0.1:8788",
                "token": operator_token,
            }),
        ),
        (
            "echo-weles-api",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
        (
            "weles-keyword-planner-model-router",
            json!({"token": generated_authority(&binary, &vault)?}),
        ),
    ];
    let mut stored = Vec::with_capacity(items.len());
    for (item, value) in &items {
        crate::credential_store::owner::store_json(
            &binary,
            &vault,
            item,
            "internal-authority",
            value,
            &json!({}),
        )
        .map_err(|error| CmdError::click(error.to_string()))?;
        stored.push(*item);
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "recreated",
                "vault": vault,
                "items": stored,
            }))?
        );
    } else {
        println!(
            "recreated {} Weles internal authority item(s) in {}",
            stored.len(),
            vault.display()
        );
    }
    Ok(())
}
