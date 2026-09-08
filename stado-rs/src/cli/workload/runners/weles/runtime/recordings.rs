//! The `weles-recordings` workload: where a host writes its recordings, in
//! the canonical registry and in the LaunchAgents that read it.

use serde_json::{json, Value};

use crate::cli::workload::plan::print_json;
use crate::cli::CmdError;
use crate::targets::ComputeTarget;

pub(crate) fn recordings_status(target: &ComputeTarget, json_output: bool) -> Result<(), CmdError> {
    let path = target
        .weles
        .as_ref()
        .and_then(|weles| weles.recordings_dir.as_deref())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} declares no recordings directory; add it to the canonical registry",
                target.name
            ))
        })?;
    if json_output {
        print_json(&json!({
            "kind": "weles-recordings",
            "target": target.name,
            "recordings_directory": path,
        }));
    } else {
        println!("{}: Weles recordings are written to {path}", target.name);
    }
    Ok(())
}

pub(crate) async fn set_weles_recordings_dir(
    target: &str,
    path: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    use serde_json::Map;

    if !std::path::Path::new(path).is_absolute() {
        return Err(CmdError::usage(
            "weles-recordings plan path must be absolute; fix the plan",
        ));
    }

    let store = crate::targets::RegistryStore::open().await?;
    let current = store.read_versioned().await?.ok_or_else(|| {
        CmdError::click("canonical registry declares no generation; publish it first")
    })?;
    let mut document: Value = serde_json::from_str(&current.content)?;
    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            CmdError::click("canonical registry declares no targets array; repair it")
        })?;
    let entry = targets
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target))
        .ok_or_else(|| {
            CmdError::click(format!(
                "target '{target}' is not declared; add it to the canonical registry"
            ))
        })?
        .as_object_mut()
        .ok_or_else(|| CmdError::click("canonical registry target is not an object; repair it"))?;

    let weles = entry
        .entry("weles")
        .or_insert_with(|| json!({"enabled": false, "actions": []}))
        .as_object_mut()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{target} declares no Weles object; add it to the canonical registry"
            ))
        })?;
    weles.insert(
        "recordings_dir".to_string(),
        Value::String(path.to_string()),
    );

    if let Some(cleanup) = entry.get_mut("disk_cleanup").and_then(Value::as_object_mut) {
        let cleaners = cleanup
            .entry("cleaners")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(
                    "disk_cleanup declares no cleaners object; repair the canonical registry",
                )
            })?;
        let cleaner = cleaners
            .entry("weles_recordings")
            .or_insert_with(|| json!({"min_age_seconds": 604800}))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(
                    "weles_recordings declares no cleaner object; repair the canonical registry",
                )
            })?;
        cleaner.insert("root".to_string(), Value::String(path.to_string()));
    }

    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let generation = store.compare_and_swap(&current.version, &payload).await?;
    if !json_output {
        println!("registry: {target} weles.recordings_dir={path} (generation {generation})");
    }

    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    let registry = crate::targets::load_registry_from_str(&payload)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let is_self = registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .is_some_and(|entry| entry.name == target);
    if !is_self {
        if json_output {
            print_json(&json!({
                "kind": "weles-recordings",
                "target": target,
                "recordings_directory": path,
                "registry_generation": generation,
                "updated_launch_agents": 0,
                "launch_agents_local": false,
            }));
        } else {
            println!(
                "{target}: registry updated; run this workload on that host to update its LaunchAgents"
            );
        }
        return Ok(());
    }

    std::fs::create_dir_all(path)?;
    let agents_dir = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| CmdError::click("HOME is not set; set it before updating LaunchAgents"))?
        .join("Library/LaunchAgents");
    let mut touched = 0usize;
    for item in std::fs::read_dir(&agents_dir)? {
        let plist = item?.path();
        let Some(name) = plist.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("com.wisent.weles-") || !name.ends_with(".plist") {
            continue;
        }
        set_plist_recordings_root(&plist, path)?;
        touched += 1;
        if !json_output {
            println!("  {name}: WELES_RECORDINGS_ROOT={path}");
        }
    }
    if json_output {
        print_json(&json!({
            "kind": "weles-recordings",
            "target": target,
            "recordings_directory": path,
            "registry_generation": generation,
            "updated_launch_agents": touched,
            "launch_agents_local": true,
        }));
    } else {
        println!("updated {touched} LaunchAgent plist(s); reload Weles agents to apply");
    }
    Ok(())
}

fn set_plist_recordings_root(plist: &std::path::Path, path: &str) -> Result<(), CmdError> {
    fn plutil(plist: &std::path::Path, args: &[&str]) -> std::io::Result<std::process::Output> {
        std::process::Command::new("/usr/bin/plutil")
            .args(args)
            .arg(plist)
            .output()
    }

    let key = "EnvironmentVariables.WELES_RECORDINGS_ROOT";
    if plutil(plist, &["-replace", key, "-string", path])?
        .status
        .success()
    {
        return Ok(());
    }
    if plutil(plist, &["-insert", key, "-string", path])?
        .status
        .success()
    {
        return Ok(());
    }
    let _ = plutil(
        plist,
        &["-insert", "EnvironmentVariables", "-xml", "<dict/>"],
    )?;
    let retry = plutil(plist, &["-insert", key, "-string", path])?;
    if retry.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&retry.stderr).trim().to_string();
    Err(CmdError::click(format!(
        "{}: failed to update WELES_RECORDINGS_ROOT: {message}",
        plist.display()
    )))
}
