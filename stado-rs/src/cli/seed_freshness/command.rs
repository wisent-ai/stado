//! The command itself: both host reads, the join, and the two output shapes.

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::seed_freshness::remote::evidence::{
    parse_marked_line, SEED_EVIDENCE_MARKER, SEED_EVIDENCE_SOURCE,
};
use crate::cli::seed_freshness::remote::skarbiec::remote_seed_state;
use crate::cli::seed_freshness::report::join::build_report;
use crate::cli::seed_freshness::report::render::render;
use crate::cli::seed_freshness::verdict::inputs::SEED_READ_UNSUPPORTED;

/// Ask the host for both halves and print the joined verdict.
///
/// Two host reads, both read-only: one Skarbiec sweep for the vault's half and
/// one node reader for the run history's half. The vault sweep is a single
/// invocation on purpose — asking per row would open the vault once per
/// account.
pub async fn authenticator_seed_freshness(
    target: &str,
    login_item: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let credential_host = crate::cli::host::credential_host(target).await?;
    let resolved = credential_host.target;
    let home = credential_host.home;
    let vault_path = credential_host.vault;
    let gnupg_home = credential_host.gnupg_home;

    let mut arguments = vec![String::from("totp-seed-state")];
    if let Some(item) = login_item {
        arguments.push(item.to_string());
    }
    let vault = remote_seed_state(
        &resolved,
        &runner,
        &home,
        &vault_path,
        &gnupg_home,
        &arguments,
    )
    .await;
    // One item asked for comes back as one object; the report always joins on
    // a list, so a single row is wrapped rather than special-cased below.
    let mut vault_unsupported = None;
    let vault = match vault {
        Ok(answer) if answer.get("rows").is_some() => answer,
        Ok(answer) => json!({"rows": [answer]}),
        // A host still running a Skarbiec without this read is reported, not
        // fatal: the sign-in history is the half nobody was reading, and it is
        // still here.
        Err(error) if error.to_string().contains("unknown command") => {
            vault_unsupported = Some(error.to_string());
            Value::Null
        }
        Err(error) => return Err(error),
    };

    let mut node = None;
    for candidate in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        let present = crate::deploy::host_channel::remote_test(
            &resolved,
            &format!("-x {candidate}"),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if present {
            node = Some(candidate);
            break;
        }
    }
    let node = node.ok_or_else(|| {
        CmdError::click(format!(
            "{}: Node.js is unavailable on this host",
            resolved.name
        ))
    })?;
    let output = crate::deploy::host_channel::run_program_with_stdin(
        &resolved,
        &[node, "-"],
        SEED_EVIDENCE_SOURCE,
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: the sign-in evidence read did not complete: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    let evidence = parse_marked_line(&output.stdout, SEED_EVIDENCE_MARKER, "sign-in evidence")?;

    let vault = match &vault_unsupported {
        None => vault,
        Some(_) => {
            // Every account the recorded history names, so the report still
            // has one row per account it has evidence about.
            let mut items: Vec<String> = evidence
                .get("attempts")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| row.get("login_item").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            items.sort();
            items.dedup();
            json!({"rows": items
                .into_iter()
                .map(|item| json!({
                    "item": item,
                    "kind": "login",
                    "seed_state": SEED_READ_UNSUPPORTED,
                }))
                .collect::<Vec<Value>>()})
        }
    };
    let mut report = build_report(&resolved.name, &vault, &evidence);
    if let Some(detail) = vault_unsupported {
        report["vault_half_unavailable"] = json!(detail);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render(&report));
    }
    Ok(())
}
