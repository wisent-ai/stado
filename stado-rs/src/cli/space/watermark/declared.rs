//! Applying a declared policy to a host, and reading back whether that host
//! is actually managed.
//!
//! Both halves exist because of the same defect. charless-mac-mini's
//! declaration was readable the whole time it was useless: `mode` was
//! `report`, its one repair could never fire, and the command that printed it
//! printed the fields and left the judgement to whoever was looking. So the
//! read here states the verdict — armed or not, and which declared policy the
//! document IS — and the write refuses anything but a policy written for this
//! host.

use serde_json::{json, Value};

use super::args::WatermarkArgs;
use super::fields::strip_nulls;
use crate::cli::space::print_json;
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::providers::local::host_memory::declaration::policies;
use crate::providers::local::host_memory::declaration::policies::automatic_verdict;

/// Every refusal here is one thing: an explicit declaration refused this
/// write. Saying so at the site keeps the classifier from reading the
/// wording and reporting a policy refusal as an unattributable failure.
fn refused(args: &WatermarkArgs, message: String, help: String) -> CmdError {
    CmdError::click(message)
        .stating(FailureCode::Refused)
        .helping(help)
        .machine_readable(args.json)
}

/// The declared policy this write applies, refused unless it is written for
/// this host and its graphical-session authorization was given at this call.
pub(super) fn declared_policy_for(
    name: &str,
    entry: &serde_json::Map<String, Value>,
    args: &WatermarkArgs,
) -> Result<Value, CmdError> {
    if args.edits_fields_by_hand() {
        return Err(CmdError::usage(
            "--policy applies one declared policy whole; drop the --memory-* flags to apply it, \
             or drop --policy to edit fields by hand",
        )
        .stating(FailureCode::Refused)
        .machine_readable(args.json));
    }
    let declared = policies::find(name).map_err(|error| {
        refused(
            args,
            error,
            format!(
                "declared policies: {}",
                policies::declared_names().join(", ")
            ),
        )
    })?;
    let platform = entry
        .get("release_platform")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let role = entry.get("role").and_then(Value::as_str);
    if !declared.fits(platform, role) {
        let fitting = policies::fitting(platform, role);
        let alternatives = if fitting.is_empty() {
            "no declared policy is written for it".to_string()
        } else {
            format!(
                "the policies written for it are {}",
                fitting
                    .iter()
                    .map(|policy| policy.name.as_str())
                    .collect::<Vec<&str>>()
                    .join(", ")
            )
        };
        return Err(refused(
            args,
            format!(
                "policy {name} is written for platforms [{}] and roles [{}], and {} is {} {}",
                declared.platforms.join(", "),
                declared.roles.join(", "),
                args.target,
                platform,
                role.unwrap_or("a host with no declared role"),
            ),
            alternatives,
        ));
    }
    // The two-declaration rule the repair itself carries, kept at the writer:
    // the catalog names the processes, and the operator authorizes ending
    // them here, for this host, in this call.
    if declared.ends_graphical_session() && !args.authorize_graphical_session {
        return Err(refused(
            args,
            format!(
                "policy {name} ends the logged-in session processes {}",
                declared.session_processes().join(", ")
            ),
            format!(
                "re-run with --authorize-graphical-session to authorize ending them on {}, or \
                 apply a policy that ends none",
                args.target
            ),
        ));
    }
    let mut document = serde_json::to_value(&declared.policy)?;
    strip_nulls(&mut document);
    Ok(document)
}

/// Print one host's declarations: the memory one together with the verdict
/// on it, and the disk one as the registry carries it.
pub(super) fn print_read(
    target: &str,
    declared: Option<Value>,
    disk: Option<Value>,
    json_output: bool,
) -> Result<(), CmdError> {
    let verdict = automatic_verdict(declared.as_ref());
    if json_output {
        return print_json(&json!({
            "target": target,
            "declared": declared.is_some(),
            "automatic": verdict,
            "memory_reclaim": declared,
            "disk_cleanup": disk,
        }));
    }
    println!(
        "{target}: {}",
        verdict
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    if let Some(reviewed) = verdict.get("reviewed_policy").and_then(Value::as_str) {
        println!("declared policy: {reviewed}");
    } else if declared.is_some() {
        println!(
            "declared policy: none of {}; this document was written by hand",
            policies::DECLARATION_PATH
        );
    }
    if let Some(policy) = declared {
        println!("memory_reclaim:");
        println!("{}", serde_json::to_string_pretty(&policy)?);
    }
    match disk {
        Some(policy) => {
            println!("disk_cleanup:");
            println!("{}", serde_json::to_string_pretty(&policy)?);
        }
        None => println!(
            "disk_cleanup: none declared; the host is measured against the reporting default \
             until `--disk-low-free-gb` and `--disk-target-free-gb` are written"
        ),
    }
    Ok(())
}
