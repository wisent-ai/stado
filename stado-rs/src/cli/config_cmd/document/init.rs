//! `config init`: write the commented template to the deployment's config
//! path, and seed the local target registry that first run needs beside it.

use crate::config_file;

use crate::cli::CmdError;

fn initialize_local_registry(home: &std::path::Path) -> Result<(), CmdError> {
    let storage_root = home.join(".stado").join("local-storage");
    std::fs::create_dir_all(&storage_root)?;
    let registry_path = storage_root.join(crate::targets::REGISTRY_BLOB);
    if registry_path.exists() {
        return Ok(());
    }

    let hostname = crate::providers::vast::system_hostname();
    let identity = crate::targets::normalize_hostname(&hostname);
    let target_name = identity
        .split('.')
        .next()
        .unwrap_or(identity.as_str())
        .to_string();
    let hostnames = if target_name == identity {
        Vec::new()
    } else {
        vec![identity]
    };
    let policy_unit = crate::providers::local::disk_cleanup::STATE_VERSION;
    let release_platform = crate::self_update::platform_triple_short()
        .map_err(|error| CmdError::click(error.to_string()))?;
    let registry = serde_json::json!({
        "schema_version": crate::targets::REGISTRY_SCHEMA_VERSION,
        "coordinators": [],
        "targets": [{
            "name": target_name,
            "kind": "local",
            "hostnames": hostnames,
            "release_platform": release_platform,
            "disk_cleanup": {
                "mode": "off",
                "check_interval_seconds": i64::try_from(
                    crate::primitives::constants::MIN_RUNTIME_BEFORE_YIELD_S
                ).expect("cleanup interval fits i64"),
                "low_free_gb": policy_unit,
                "target_free_gb": policy_unit.saturating_add(policy_unit),
                "max_bytes_per_pass": crate::providers::local::disk_cleanup::GIB,
                "max_items_per_pass": policy_unit,
                "max_scan_items": policy_unit,
                "cleaners": {}
            }
        }]
    });
    crate::targets::validate_registry(&registry).map_err(|error| {
        CmdError::click(format!("generated local registry is invalid: {error}"))
    })?;
    let body = format!("{}\n", serde_json::to_string_pretty(&registry)?);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&registry_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    std::io::Write::write_all(&mut file, body.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// `config init`: write the commented template to ~/.stado/config.json.
pub(in crate::cli::config_cmd) fn init() -> Result<(), CmdError> {
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let path = std::path::Path::new(&home)
        .join(".stado")
        .join("config.json");
    if path.exists() {
        return Err(CmdError::click(format!(
            "config file already exists: {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(&config_file::template())?;
    initialize_local_registry(std::path::Path::new(&home))?;
    std::fs::write(&path, format!("{body}\n"))?;
    println!("{}", path.display());
    Ok(())
}
