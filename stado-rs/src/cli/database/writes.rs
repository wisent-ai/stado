//! The single writer behind every verb: one validated, atomic mutation of
//! `database_api.databases`, the name rule each verb enforces before it, and
//! the report printed once the write lands.

use serde_json::{json, Value};

use crate::cli::CmdError;

/// Load the config file, apply one mutation to `database_api.databases`,
/// refuse anything the plane's own parser rejects, and write atomically.
pub(super) fn mutate_databases<F>(mutation: F) -> Result<Value, CmdError>
where
    F: FnOnce(&mut serde_json::Map<String, Value>) -> Result<(), String>,
{
    let path = crate::config_file::config_path()
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::click("no config file exists; run: stado config init")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let original = std::fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&original).map_err(CmdError::from)?;
    if !document.is_object() {
        return Err(CmdError::click("config file must contain a JSON object")
            .stating(crate::primitives::failure::FailureCode::Config));
    }

    let entry = document
        .as_object_mut()
        .expect("checked above")
        .entry("database_api".to_string())
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        return Err(CmdError::click("database_api must be an object")
            .stating(crate::primitives::failure::FailureCode::Config));
    }
    let databases = entry
        .as_object_mut()
        .expect("checked above")
        .entry("databases".to_string())
        .or_insert_with(|| json!({}));
    let map = databases.as_object_mut().ok_or_else(|| {
        CmdError::click("database_api.databases must be an object")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    mutation(map)?;
    // The parser refuses an empty map, so a removal that empties the plane
    // collapses the section instead of leaving a configuration nothing can
    // validate.
    if map.is_empty() {
        document
            .as_object_mut()
            .expect("checked above")
            .remove("database_api");
    }

    // The plane's parser is the authority on shape; run it before the whole
    // document's validation so the refusal names the database, not an
    // unrelated section the generic validator happened to reach first. A
    // document that no longer carries the section at all has nothing for
    // this plane to reject.
    if let Some(databases) = document
        .get("database_api")
        .and_then(|section| section.get("databases"))
    {
        if let Err(problems) = crate::config::parse_database_api_databases(Some(databases)) {
            return Err(CmdError::click(format!(
                "rejected, config unchanged: {}",
                problems.join("; ")
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }
    }

    let problems = crate::config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "rejected, config unchanged: {}",
            problems.join("; ")
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }

    let body = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let temporary = std::path::PathBuf::from(format!("{}.database-setting", path.display()));
    std::fs::write(&temporary, body)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    Ok(document)
}

pub(super) fn canonical_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(super) fn report_mutation(json_output: bool, report: Value) -> Result<(), CmdError> {
    crate::cli::print_answer(&report, json_output)
}

/// This machine's `database_api` block, as its config file holds it.
fn local_block() -> Result<Value, CmdError> {
    let path = crate::config_file::config_path()
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::click("no config file exists; run: stado config init")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let document: Value =
        serde_json::from_str(&std::fs::read_to_string(&path)?).map_err(|error| {
            CmdError::click(format!("{}: {error}", path.display()))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    document.get("database_api").cloned().ok_or_else(|| {
        CmdError::click(format!("{} declares no database_api", path.display()))
            .stating(crate::primitives::failure::FailureCode::Config)
    })
}

/// Make HOST's `database_api` block equal to this machine's, then reconcile
/// SERVICE so its running process reads it. HOST's copy is read whole from
/// its config file: `config show` prints the resolved struct without it.
/// With `check`, nothing is written and a difference is a refusal.
pub(super) async fn push(
    host: &str,
    service: &str,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let local = local_block()?;
    let target = crate::cli::canonical_host(host).await?;
    let fetched = crate::deploy::service_file_fetch::fetch_file(
        &target,
        "$HOME/.config/stado/config.json",
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(CmdError::from)?;
    if fetched.integrity != crate::deploy::service_file_fetch::INTEGRITY_VERIFIED {
        return Err(CmdError::click(format!(
            "{host}'s config file did not arrive intact: {}",
            fetched.integrity
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let remote: Value = serde_json::from_slice(&fetched.content).map_err(|error| {
        CmdError::click(format!("{host}'s config file: {error}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let agrees = remote.get("database_api") == Some(&local);
    let mut restarted = Value::Null;
    let status = match (agrees, check) {
        (true, _) => "in agreement",
        (false, true) => "would push",
        (false, false) => {
            crate::cli::host::write_host_config(host, "database_api", &local.to_string()).await?;
            // The unit's restart reports go into this command's one receipt:
            // a second document on stdout would make `--json` unparseable.
            restarted = crate::cli::service::restart_quietly(service, Some(host)).await?;
            "pushed"
        }
    };
    report_mutation(
        json_output,
        json!({ "host": host, "service": service, "database_api": status, "restarted": restarted }),
    )?;
    if check && !agrees {
        return Err(CmdError::refused(format!(
            "{host}'s database_api differs from this machine's"
        )));
    }
    Ok(())
}
