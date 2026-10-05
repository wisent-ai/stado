//! `stado credentials grant rebind --host OWNER --token-file PATH`: bind the
//! `stado` grant back to the bearer file every Stado on the fleet holds.
//!
//! Skarbiec stores the hash of the token file a grant was issued with. When
//! that record and the file disagree (a vault owner's disk filling mid-write
//! leaves every read after it answering "consumer not authorized to read
//! item field"), no Stado command can read a credential
//! again, and the only repair was a hand-typed `skarbiec grant issue` on the
//! owner. This is that repair as a command: the grant keeps exactly its
//! capabilities, audience and remaining lifetime, and only the bearer it is
//! bound to changes, so every copy of the file on the fleet reads again.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::secrets::vault::mirror::remote_skarbiec_json;
use crate::cli::CmdError;

const CONSUMER: &str = "stado";

/// `action:item[#field]`, the spelling `grant issue --capabilities` reads.
fn capability(entry: &Value) -> Option<String> {
    let action = entry.get("action")?.as_str()?;
    let item = entry.get("item")?.as_str()?;
    Some(match entry.get("field").and_then(Value::as_str) {
        Some(field) => format!("{action}:{item}#{field}"),
        None => format!("{action}:{item}"),
    })
}

/// Whether `token_file` on the owner opens the grant's first capability.
async fn verifies(host: &str, first: &Value, token_file: &str) -> Result<(bool, Value), CmdError> {
    let mut probe = vec![
        "grant".to_string(),
        "verify".into(),
        CONSUMER.into(),
        first["item"].as_str().unwrap_or_default().into(),
        "--action".into(),
        first["action"].as_str().unwrap_or_default().into(),
        "--token-file".into(),
        token_file.into(),
    ];
    if let Some(field) = first.get("field").and_then(Value::as_str) {
        probe.extend(["--field".into(), field.into()]);
    }
    let (_, verdict) = remote_skarbiec_json(host, &probe).await?;
    Ok((
        verdict.get("allowed").and_then(Value::as_bool) == Some(true),
        verdict,
    ))
}

pub async fn rebind(host: &str, token_file: &str, json_output: bool) -> Result<(), CmdError> {
    if !token_file.starts_with('/') {
        return Err(CmdError::usage(
            "--token-file must be an absolute path on the vault host",
        ));
    }
    let (target, listing) = remote_skarbiec_json(host, &["grant".into(), "list".into()]).await?;
    let grant = listing
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["consumer"] == CONSUMER))
        .cloned()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: the vault holds no stado grant to rebind",
                target.name
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let entries = grant["capabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let first = entries.first().cloned().ok_or_else(|| {
        CmdError::click(format!(
            "{}: the stado grant carries no capability",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let (already, _) = verifies(host, &first, token_file).await?;
    if already {
        return Err(CmdError::refused(format!(
            "{}: {token_file} already opens the stado grant; nothing was changed",
            target.name
        )));
    }
    let capabilities: BTreeSet<String> = entries.iter().filter_map(capability).collect();
    if capabilities.len() != entries.len() {
        return Err(CmdError::click(format!(
            "{}: a stado capability is malformed; nothing was changed",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let audience = grant["audience"]
        .as_str()
        .ok_or_else(|| {
            CmdError::click(format!("{}: the stado grant has no audience", target.name))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?
        .to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| CmdError::click(error.to_string()))?
        .as_secs();
    let expires_at = grant["expires_at"].as_u64().ok_or_else(|| {
        CmdError::click(format!(
            "{}: the stado grant has no numeric expiry",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let Some(lifetime) = crate::credential_store::grant::GrantLifetime::left(expires_at, now)
    else {
        return Err(CmdError::refused(format!(
            "{}: the stado grant has expired; rebinding does not renew it",
            target.name
        )));
    };
    let mut args: Vec<String> = vec![
        "grant".into(),
        "issue".into(),
        CONSUMER.into(),
        "--capabilities".into(),
        capabilities.iter().cloned().collect::<Vec<_>>().join(","),
        "--token-file".into(),
        token_file.into(),
    ];
    args.extend(lifetime.args());
    args.extend([
        "--audience".into(),
        audience,
        "--replace-capabilities".into(),
    ]);
    remote_skarbiec_json(host, &args).await?;
    let (now_verifies, verdict) = verifies(host, &first, token_file).await?;
    if !now_verifies {
        return Err(CmdError::click(format!(
            "{}: the stado grant was re-issued but {token_file} still does not open it: {verdict}",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let report = json!({
        "host": target.name, "consumer": CONSUMER, "token_file": token_file,
        "capabilities": capabilities.len(), "expires_at": expires_at, "verified": true,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}: stado grant bound to {token_file} again ({} capabilities, same expiry); verified",
            target.name,
            capabilities.len()
        );
    }
    Ok(())
}
