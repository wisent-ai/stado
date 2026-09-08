//! `config migrate`: raise a legacy document to the current root schema,
//! keeping the exact prior file beside it.

use serde_json::Value;

use crate::config_file;

use crate::cli::CmdError;

/// Add the current root schema to a legacy config while preserving the exact
/// prior document beside it. Future schemas are never rewritten or downgraded.
pub(in crate::cli::config_cmd) fn migrate() -> Result<(), CmdError> {
    let path = config_file::config_path()
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists to migrate"))?;
    let raw = std::fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&raw)?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| CmdError::click("config file must contain a JSON object"))?;
    match root.get("schema_version").and_then(Value::as_u64) {
        Some(version) if version == u64::from(config_file::SCHEMA_VERSION) => {
            println!(
                "config already uses schema_version {} ({})",
                config_file::SCHEMA_VERSION,
                path.display()
            );
            return Ok(());
        }
        Some(version) => {
            return Err(CmdError::click(format!(
                "cannot migrate config schema_version {version}; this binary supports {}",
                config_file::SCHEMA_VERSION
            )));
        }
        None => {}
    }

    root.insert(
        "schema_version".to_string(),
        Value::from(config_file::SCHEMA_VERSION),
    );
    let migrated = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let backup = std::path::PathBuf::from(format!("{}.before-schema-migration", path.display()));
    let mut backup_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(|error| {
            CmdError::click(format!(
                "cannot preserve config at {}: {error}",
                backup.display()
            ))
        })?;
    std::io::Write::write_all(&mut backup_file, raw.as_bytes())?;

    let temporary = std::path::PathBuf::from(format!("{}.migrating", path.display()));
    std::fs::write(&temporary, migrated)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    println!(
        "migrated config to schema_version {}; previous file: {}",
        config_file::SCHEMA_VERSION,
        backup.display()
    );
    Ok(())
}
