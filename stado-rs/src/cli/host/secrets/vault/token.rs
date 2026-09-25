use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::releases::release_component;
use crate::cli::host::secrets::vault::mirror::read::remote_skarbiec_json_at;
use crate::cli::host::secrets::vault::vault_word;

/// Mint a bounded bearer, or register an existing owner-vault field on TARGET.
///
/// Existing-bearer bytes stay on the target and never enter argv or the report.
/// `raw_token` exposes only a newly generated bearer for a secret-store pipe;
/// `token_file_name` instead persists and reuses it on the target.
#[allow(clippy::too_many_arguments)]
pub async fn vault_token_mint(
    target: &str,
    consumer: &str,
    capabilities: &str,
    audience: &str,
    ttl_seconds: u64,
    replace_capabilities: bool,
    token_item: Option<&str>,
    token_field: &str,
    raw_token: bool,
    token_file_name: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("consumer", consumer)?;
    vault_word("audience", audience)?;
    if let Some(item) = token_item {
        vault_word("token item", item)?;
        vault_word("token field", token_field)?;
        if raw_token {
            return Err(CmdError::usage(
                "--raw-token cannot be used with an existing --token-item",
            ));
        }
    }
    if raw_token && json_output {
        return Err(CmdError::usage(
            "--raw-token and --json cannot be used together",
        ));
    }
    if let Some(name) = token_file_name {
        release_component("token file name", name)?;
        if raw_token {
            return Err(CmdError::usage(
                "--raw-token and --token-file-name cannot be used together",
            ));
        }
    }
    if capabilities.is_empty()
        || !capabilities
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/,:#".contains(&byte))
    {
        return Err(CmdError::usage(
            "capabilities must be a comma-separated list of exact action:item[#field] values",
        ));
    }
    let mut arguments = vec![
        String::from("grant"),
        String::from("issue"),
        consumer.to_string(),
        String::from("--capabilities"),
        capabilities.to_string(),
        String::from("--audience"),
        audience.to_string(),
        String::from("--ttl-seconds"),
        ttl_seconds.to_string(),
    ];
    if replace_capabilities {
        arguments.push(String::from("--replace-capabilities"));
    }
    let token_source = token_item.map(|item| (item, token_field));
    let (resolved, mut report) =
        remote_skarbiec_json_at(target, &arguments, None, token_source, token_file_name).await?;
    if token_source.is_none() && token_file_name.is_none() {
        let token = report
            .get("token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{}: Skarbiec grant issue returned no bearer",
                    resolved.name
                ))
            })?;
        if raw_token {
            println!("{token}");
            return Ok(());
        }
    }
    if let Some(object) = report.as_object_mut() {
        object.remove("token");
    }
    if json_output {
        let mut metadata = json!({
            "target": resolved.name,
            "status": if token_source.is_some() { "token_registered" } else { "token_minted" },
            "skarbiec": report,
        });
        if let Some((item, field)) = token_source {
            metadata["token_source"] = json!({ "item": item, "field": field });
        }
        println!("{}", serde_json::to_string_pretty(&metadata)?);
    } else {
        let operation = if token_source.is_some() {
            "registered"
        } else {
            "minted"
        };
        println!(
            "{}: token {operation} for {consumer} with audience {audience}",
            resolved.name
        );
        if let Some(path) = report.get("token_file").and_then(Value::as_str) {
            println!("Bearer file: {path}");
        }
    }
    Ok(())
}

/// Deliver a bootstrap bearer without minting, renewing, or widening a grant.
/// Both declared vaults must already hold the same owner and complete grant.
pub async fn vault_token_sync(
    from_host: &str,
    target: &str,
    consumer: &str,
    source_token_file: &str,
    token_file: &str,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    use crate::cli::host::machine::users::credentials::credential_host;
    use crate::deploy::host_channel;
    use crate::primitives::failure::FailureCode;

    vault_word("consumer", consumer).map_err(|error| error.machine_readable(json_output))?;
    if source_token_file.trim().is_empty() || token_file.trim().is_empty() {
        return Err(
            CmdError::usage("source and destination token files must be named")
                .machine_readable(json_output),
        );
    }
    // Resolve both declarations before reading any bearer. The payload only
    // crosses encrypted host channels in memory; it is never a CLI argument.
    let source = credential_host(from_host)
        .await
        .map_err(|error| error.machine_readable(json_output))?;
    let destination = credential_host(target)
        .await
        .map_err(|error| error.machine_readable(json_output))?;
    let runner = crate::deploy::production_runner();
    let program = include_str!("../../../../host_payloads/vault_token/sync.py");
    let exported = host_channel::run_program(
        &source.target,
        &[
            "/usr/bin/python3",
            "-c",
            program,
            "export",
            &source.vault,
            consumer,
            source_token_file,
        ],
        &runner,
    )
    .await
    .map_err(|error| {
        CmdError::click(format!(
            "{}: token export failed: {error}",
            source.target.name
        ))
        .machine_readable(json_output)
    })?;
    if !exported.ok() {
        return Err(CmdError::click(format!(
            "{}: token export refused: {}",
            source.target.name,
            host_channel::last_error_line(&exported, "host token export failed")
        ))
        .stating(FailureCode::Refused)
        .machine_readable(json_output));
    }
    let installed = host_channel::run_program_with_stdin(
        &destination.target,
        &[
            "/usr/bin/python3",
            "-c",
            program,
            if check { "check" } else { "install" },
            &destination.vault,
            consumer,
            token_file,
        ],
        &exported.stdout,
        &runner,
    )
    .await
    .map_err(|error| {
        CmdError::click(format!(
            "{}: token delivery failed: {error}",
            destination.target.name
        ))
        .machine_readable(json_output)
    })?;
    drop(exported);
    if !installed.ok() {
        return Err(CmdError::click(format!(
            "{}: token delivery refused: {}",
            destination.target.name,
            host_channel::last_error_line(&installed, "host token delivery failed")
        ))
        .stating(FailureCode::Refused)
        .machine_readable(json_output));
    }
    let mut report: Value = serde_json::from_str(installed.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "token delivery returned unreadable metadata: {error}"
        ))
        .machine_readable(json_output)
    })?;
    report["target"] = json!(destination.target.name);
    report["source_host"] = json!(source.target.name);
    let status = report["status"]
        .as_str()
        .filter(|status| {
            matches!(
                *status,
                "token_synced" | "token_unchanged" | "token_checked"
            )
        })
        .ok_or_else(|| {
            CmdError::click("token delivery returned no recognized outcome")
                .machine_readable(json_output)
        })?;
    let delivered_path = report["skarbiec"]["token_file"]
        .as_str()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            CmdError::click("token delivery returned no verified file")
                .machine_readable(json_output)
        })?;
    if report["skarbiec"]["ok"] != true || report["skarbiec"]["consumer"] != consumer {
        return Err(
            CmdError::click("token delivery did not verify the requested consumer")
                .machine_readable(json_output),
        );
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}: {} for {consumer}; source {}; grants unchanged",
            destination.target.name, status, source.target.name,
        );
        println!("Bearer file: {delivered_path}");
    }
    Ok(())
}
