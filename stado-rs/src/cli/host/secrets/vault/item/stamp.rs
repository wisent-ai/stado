//! Stamp payload fingerprints on the host that owns the vault.
//!
//! Skarbiec 0.3.12 carries `stamp-fingerprints`, which describes every item
//! written before payload fingerprints existed so that the duplicate report
//! and the exact-duplicate refusal cover the whole vault. Running it on a
//! machine that holds a replica is wasted work: on 2026-09-21 the pass
//! stamped 658 items in this laptop's `~/.stado/skarbiec.vault.json`, the
//! owner's next sync replaced the file, and the report was blind again within
//! the hour. It is the same shape as a grant written by hand to a replica.
//!
//! The owner key and the canonical vault live on one host, so the pass runs
//! there and nowhere else, and this reports what that host answered.

use serde_json::{json, Value};

use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::CmdError;

/// The verb's own name, matched as a substring of the binary's strings. A
/// build that predates the pass carries it nowhere, and a build that has it
/// carries it in the advertised command list. Substring, not a whole line:
/// rustc packs string literals into one unterminated blob, so a whole-line
/// match reports absent on a binary that has the verb.
const USAGE_MARK: &str = "stamp-fingerprints";

pub async fn stamp_vault_fingerprints(
    target: &str,
    apply: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let credential_host = credential_host(target).await?;
    let resolved = credential_host.target;
    let home = credential_host.home;
    let vault = credential_host.vault;
    let gnupg_home = credential_host.gnupg_home;
    let runner = crate::deploy::production_runner();
    let skarbiec = crate::cli::host::release_managed_skarbiec(&resolved, &runner, &home).await?;

    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: the vault at {vault} could not be stamped: {detail}",
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
    let capable = crate::deploy::host_channel::run_command(
        &resolved,
        &format!(
            "strings -a {} 2>/dev/null | grep -q {}",
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(USAGE_MARK)
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !capable.ok() {
        return Err(refused(format!(
            "the Skarbiec build at {skarbiec} predates the stamping pass; deliver 0.3.12 or later \
             to this host first"
        )));
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
    let verb = if apply {
        "stamp-fingerprints --apply"
    } else {
        "stamp-fingerprints"
    };
    let stamped = read_json(&resolved, &gnupg_home, &vault, &skarbiec, verb, &runner)
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

    let unreadable = stamped
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
                "pass": stamped,
                "after": after,
            }))?
        );
    } else {
        println!(
            "{}: {} of {} items were comparable before, {} after; {} {}, {unreadable} unreadable",
            resolved.name,
            field(&coverage, "compared"),
            field(&coverage, "active_items"),
            field(&after, "compared"),
            field(&stamped, "stamped"),
            if apply { "stamped" } else { "would be stamped" },
        );
        println!(
            "{}: duplicate groups {} -> {}",
            resolved.name,
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
