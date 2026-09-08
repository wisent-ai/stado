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
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists; run: stado config init"))?;
    let original = std::fs::read_to_string(&path)?;
    let mut document: Value =
        serde_json::from_str(&original).map_err(|error| CmdError::click(error.to_string()))?;
    if !document.is_object() {
        return Err(CmdError::click("config file must contain a JSON object"));
    }

    let entry = document
        .as_object_mut()
        .expect("checked above")
        .entry("database_api".to_string())
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        return Err(CmdError::click("database_api must be an object"));
    }
    let databases = entry
        .as_object_mut()
        .expect("checked above")
        .entry("databases".to_string())
        .or_insert_with(|| json!({}));
    let map = databases
        .as_object_mut()
        .ok_or_else(|| CmdError::click("database_api.databases must be an object"))?;
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
            )));
        }
    }

    let problems = crate::config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "rejected, config unchanged: {}",
            problems.join("; ")
        )));
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
    let _ = json_output;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
