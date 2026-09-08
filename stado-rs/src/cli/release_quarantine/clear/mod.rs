//! `stado release quarantine clear` — retire exactly one quarantined digest,
//! with the backup and the audit line that make the rewrite accountable.

use chrono::Utc;
use serde_json::json;

use crate::cli::CmdError;
use crate::deploy::{host_channel, production_runner, shlex_quote};
use crate::release_agent;
use crate::release_control::sha256_bytes;

use super::control::{canonical_control, compute_target, resolve_target};
use super::remote::remote_read;
use super::splice;
use super::QuarantineClearArgs;

mod record;
mod script;

use record::{actor, audit_path, stamp};
use script::CLEAR_TEMPLATE;

pub(super) async fn clear(args: &QuarantineClearArgs) -> Result<(), CmdError> {
    let reason = args.reason.trim();
    if reason.is_empty() {
        return Err(CmdError::usage(
            "--reason must say why this digest is being retried",
        ));
    }
    let digest = args.digest.trim().to_ascii_lowercase();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CmdError::usage(
            "--digest must be the 64-character sha256 hex digest quarantine list prints",
        ));
    }
    let control = canonical_control().await?;
    let (target_name, _, target_policy) =
        resolve_target(&control, &args.product, Some(&args.target))?;
    let path = release_agent::host_state_path(&target_policy.state_dir, &args.product);
    let host = compute_target(&target_name).await?;
    let payload = remote_read(&host, &path).await?.ok_or_else(|| {
        CmdError::click(format!(
            "{target_name} has no rollout state at {path}: nothing is quarantined there"
        ))
    })?;
    let mut state =
        release_agent::parse_state_document(payload.as_bytes(), &args.product, &target_name, &path)
            .map_err(CmdError::click)?;
    let Some(record) = state.quarantined.remove(&digest) else {
        return Err(CmdError::click(format!(
            "{digest} is not quarantined for {} on {target_name}",
            args.product
        )));
    };
    // `phase` and `updated_at` stay exactly as the agent left them. They are
    // the agent's account of its own last tick, and a tick is precisely what
    // this command does not perform; rewriting them would have `release status`
    // report a reconciliation that never ran.
    let document = release_agent::state_document_bytes(&state).map_err(CmdError::click)?;
    let audited_at = Utc::now();
    let audit = audit_path(&target_policy.state_dir, &args.product);
    let backup = format!("{path}.quarantine-backup-{}", stamp());
    let staging = format!(
        "{}/.{}.json.stado-quarantine-{}",
        target_policy.state_dir,
        args.product,
        uuid::Uuid::new_v4().simple()
    );
    // The agent's own reason and timestamp go into the record because clearing
    // the entry deletes them from the state file, and an audit trail that
    // destroys the evidence for the change it documents is decoration.
    let mut line = serde_json::to_vec(&json!({
        "actor": actor(),
        "host": target_name,
        "product": args.product,
        "digest": digest,
        "reason": reason,
        "audited_at": audited_at.to_rfc3339(),
        "quarantine_reason": record.reason,
        "quarantined_at": record.quarantined_at.to_rfc3339(),
        "state_backup": backup,
    }))?;
    // A newline inside the record would split one clear across two JSONL rows.
    line.retain(|byte| *byte != b'\n');
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    let script = splice(
        CLEAR_TEMPLATE,
        &[
            ("@STATE@", &shlex_quote(&path)),
            ("@BACKUP@", &shlex_quote(&backup)),
            ("@STAGING@", &shlex_quote(&staging)),
            ("@AUDIT@", &shlex_quote(&audit)),
            (
                "@EXPECTED_LIVE@",
                &shlex_quote(&sha256_bytes(payload.as_bytes())),
            ),
            ("@EXPECTED_NEXT@", &shlex_quote(&sha256_bytes(&document))),
            ("@DOCUMENT@", &shlex_quote(&BASE64.encode(&document))),
            ("@RECORD@", &shlex_quote(&BASE64.encode(&line))),
        ],
    );
    let runner = production_runner();
    let output = host_channel::run_script(&host, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target_name}: rollout state was not changed: {}",
            host_channel::last_error_line(&output, "remote quarantine clear failed")
        )));
    }
    let committed = output
        .stdout
        .lines()
        .any(|line| host_channel::marker_fields(line).get(1) == Some(&"committed"));
    if !committed {
        return Err(CmdError::click(format!(
            "{target_name}: host exited clean without confirming the rewrite of {path}"
        )));
    }
    let report = json!({
        "product": args.product,
        "target": target_name,
        "digest": digest,
        "cleared": true,
        "reason": reason,
        "audited_at": audited_at.to_rfc3339(),
        "state_backup": backup,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "cleared {digest} for {} on {target_name}\n  it was quarantined at {} because: {}\n  previous state backed up to {backup}\n  audited in {audit}\n  nothing was started, stopped or restarted; the release agent rolls this digest out on its next tick",
            args.product,
            record.quarantined_at.to_rfc3339(),
            record.reason.lines().next().unwrap_or(&record.reason),
        );
    }
    Ok(())
}
