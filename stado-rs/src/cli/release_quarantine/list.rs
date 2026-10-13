//! `stado release quarantine list` — what this host refuses to roll out, and
//! which of those refusals is the digest the registry currently wants.

use serde_json::{json, Value};

use crate::cli::reporting::table;
use crate::cli::CmdError;
use crate::release_agent;
use crate::release_control::{ProductReleasePolicy, ReleaseTargetPolicy};

use super::control::{canonical_control, compute_target, resolve_target};
use super::remote::remote_host_state;
use super::QuarantineListArgs;

/// The digest the registry currently wants on this target's platform.
fn desired_digest<'a>(
    policy: &'a ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
) -> Option<&'a str> {
    policy
        .desired
        .as_ref()?
        .artifacts
        .get(&target.platform)
        .map(|artifact| artifact.artifact_sha256.as_str())
}

pub(super) async fn list(args: &QuarantineListArgs) -> Result<(), CmdError> {
    let control = canonical_control().await?;
    let (target_name, policy, target_policy) =
        resolve_target(&control, &args.product, args.target.as_deref())?;
    let desired = desired_digest(policy, target_policy);
    let path = release_agent::host_state_path(&target_policy.state_dir, &args.product);
    let host = compute_target(&target_name).await?;
    let state = remote_host_state(&host, &target_policy.state_dir, &args.product).await?;
    let mut entries = Vec::new();
    if let Some(state) = state.as_ref() {
        for (digest, record) in &state.quarantined {
            // The same derivation `release doctor` uses, on the record
            // itself, so the two commands cannot name one digest two things.
            let classified = record.classification();
            entries.push(json!({
                "digest": digest,
                "reason": record.reason,
                "quarantined_at": record.quarantined_at.to_rfc3339(),
                "is_desired_digest": desired == Some(digest.as_str()),
                "cause": classified.cause.as_str(),
                "evidence": classified.evidence,
                "agent_retires": !classified.cause.holds_the_candidate(),
            }));
        }
    }
    let report = json!({
        "product": args.product,
        "target": target_name,
        "entries": entries,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if state.is_none() {
        // "Nobody looked" printed as "nothing is there" is the one rendering
        // this must never produce: an absent state file means the agent has
        // never reconciled this product here, which is a different problem.
        println!("{target_name} has no rollout state at {path}");
        return Ok(());
    }
    if entries.is_empty() {
        println!("{} on {target_name}: nothing quarantined", args.product);
        return Ok(());
    }
    // Each cell is the entry's own value or names the field it lacks; a dash
    // read as a recorded value.
    let cell = |entry: &Value, field: &str| match entry[field].as_str() {
        Some(value) => value.to_string(),
        None => format!("(no {field})"),
    };
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            let role = if entry["is_desired_digest"] == json!(true) {
                "desired"
            } else {
                "not desired"
            };
            let reason = match entry["reason"]
                .as_str()
                .and_then(|reason| reason.lines().next())
            {
                Some(line) => line.to_string(),
                None => "(no reason)".to_string(),
            };
            vec![
                cell(entry, "digest"),
                role.to_string(),
                cell(entry, "quarantined_at"),
                cell(entry, "cause"),
                reason,
            ]
        })
        .collect();
    table::print(
        &["DIGEST", "ROLE", "QUARANTINED AT", "CAUSE", "REASON"],
        &rows,
    );
    Ok(())
}
