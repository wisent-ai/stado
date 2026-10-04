//! Bring the vault to Skarbiec's current schema on the host that owns it.
//!
//! `skarbiec upgrade` is Skarbiec's one idempotent schema pass: the v2
//! envelope, an `item_uid` on every item and a payload fingerprint on every
//! active item, so the duplicate report and the exact-duplicate refusal cover
//! the whole vault. Running it on a machine that holds a replica is wasted
//! work: the pass changes every item in the replica, the owner's next sync
//! replaces the file, and the vault is behind again within the hour. It is
//! the same shape as a grant written by hand to a replica.
//!
//! The owner key and the canonical vault live on one host, so the pass runs
//! there and nowhere else, and this reports what that host answered. A
//! Skarbiec build without the verb refuses it as an unknown command, and that
//! refusal is the error reported here.

use serde_json::{json, Value};

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::CmdError;

pub async fn upgrade_vault(target: &str, apply: bool, json_output: bool) -> Result<(), CmdError> {
    let credential_host = credential_host(target).await?;
    let resolved = credential_host.target;
    let home = credential_host.home;
    let vault = credential_host.vault;
    let gnupg_home = credential_host.gnupg_home;
    let runner = crate::deploy::production_runner();
    let skarbiec = crate::cli::host::release_managed_skarbiec(&resolved, &runner, &home).await?;

    let refused = |detail: String| {
        CmdError::refused(format!(
            "{}: the vault at {vault} could not be upgraded: {detail}",
            resolved.name
        ))
    };
    if !crate::deploy::host_channel::remote_test(
        &resolved,
        &format!("-x {}", crate::deploy::shlex_quote(&skarbiec)),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Err(refused(format!("no Skarbiec binary at {skarbiec}")));
    }
    if !crate::deploy::host_channel::remote_test(
        &resolved,
        &format!("-f {}", crate::deploy::shlex_quote(&vault)),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Err(refused(format!("no vault at {vault}")));
    }

    let coverage = read_json(
        &resolved,
        &gnupg_home,
        &vault,
        &skarbiec,
        "duplicates",
        &runner,
    )
    .await
    .map_err(refused)?;
    let verb = if apply { "upgrade --apply" } else { "upgrade" };
    let pass = read_json(&resolved, &gnupg_home, &vault, &skarbiec, verb, &runner)
        .await
        .map_err(refused)?;
    let after = read_json(
        &resolved,
        &gnupg_home,
        &vault,
        &skarbiec,
        "duplicates",
        &runner,
    )
    .await
    .map_err(refused)?;

    let fingerprints = pass.get("fingerprints").cloned().unwrap_or(Value::Null);
    let unreadable = fingerprints
        .get("unreadable")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "vault": vault,
                "applied": apply,
                "before": coverage,
                "pass": pass,
                "after": after,
            }))?
        );
    } else {
        println!(
            "{}: envelope {} -> {} ({}); item_uids {} {}; fingerprints {} {}, {unreadable} unreadable",
            resolved.name,
            field(&pass["envelope"], "from"),
            field(&pass["envelope"], "to"),
            if pass["envelope"]["needed"].as_bool() == Some(true) { "migration needed" } else { "current" },
            field(&pass["item_uids"], if apply { "stamped" } else { "missing" }),
            if apply { "stamped" } else { "missing" },
            field(&fingerprints, "stamped"),
            if apply { "stamped" } else { "would be stamped" },
        );
        println!(
            "{}: {} of {} items were comparable before, {} after; duplicate groups {} -> {}",
            resolved.name,
            field(&coverage, "compared"),
            field(&coverage, "active_items"),
            field(&after, "compared"),
            field(&coverage, "groups"),
            field(&after, "groups")
        );
    }
    Ok(())
}

fn field(document: &Value, name: &str) -> String {
    document
        .get(name)
        .map(|value| value.to_string())
        .unwrap_or_else(|| String::from("-"))
}

/// Runs one Skarbiec read or pass on the owner and parses its JSON. The
/// binary's own stdout is the report: this adds no interpretation of its own,
/// so a future field arrives here without a change.
async fn read_json(
    resolved: &crate::targets::ComputeTarget,
    gnupg_home: &str,
    vault: &str,
    skarbiec: &str,
    verb: &str,
    runner: &crate::deploy::Runner,
) -> Result<Value, String> {
    let answered = crate::deploy::host_channel::run_command(
        resolved,
        &format!(
            "GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} {verb}",
            crate::deploy::shlex_quote(gnupg_home),
            crate::deploy::shlex_quote(vault),
            crate::deploy::shlex_quote(skarbiec),
        ),
        runner,
    )
    .await
    .map_err(|error| error.to_string())?;
    if !answered.ok() {
        return Err(crate::deploy::host_channel::last_error_line(
            &answered,
            &format!("remote `skarbiec {verb}` failed"),
        ));
    }
    serde_json::from_str(answered.stdout.trim())
        .map_err(|error| format!("remote `skarbiec {verb}` printed no readable JSON: {error}"))
}
