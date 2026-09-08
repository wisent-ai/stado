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
        if token_item.is_some() {
            return Err(CmdError::usage(
                "--token-item and --token-file-name cannot be used together",
            ));
        }
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
        String::from("token-mint"),
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
                    "{}: Skarbiec token-mint returned no bearer",
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
