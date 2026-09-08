use serde_json::Value;

use crate::cli::CmdError;

/// Persist TARGET's NVIDIA board power cap in the canonical registry and apply
/// it immediately. The local agent keeps reconciling the declaration, including
/// after driver resets and host reboots.
pub async fn gpu_power_limit(target: &str, watts: u32, json: bool) -> Result<(), CmdError> {
    if watts == 0 {
        return Err(CmdError::usage("WATTS must be a positive integer"));
    }

    let store = crate::targets::RegistryStore::open().await?;
    let current = store
        .read_versioned()
        .await?
        .ok_or_else(|| CmdError::click("canonical registry generation unavailable"))?;
    let mut document: Value = serde_json::from_str(&current.content)?;
    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let entry = targets
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target))
        .ok_or_else(|| CmdError::click(format!("target not in registry: {target}")))?
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target must be an object"))?;
    entry.insert("gpu_power_limit_watts".to_string(), Value::from(watts));
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let registry = crate::targets::load_registry_from_str(&payload)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let resolved = registry
        .lookup(target)
        .cloned()
        .ok_or_else(|| CmdError::click(format!("target not in registry: {target}")))?;
    let generation = store.compare_and_swap(&current.version, &payload).await?;

    let script = format!(
        r#"set -eu
nvidia_smi=$(command -v nvidia-smi)
if [ -z "$nvidia_smi" ]; then
  printf '%s\n' 'nvidia-smi is unavailable' >&2
  exit 1
fi
indices=$("$nvidia_smi" --query-gpu=index --format=csv,noheader,nounits)
if [ -z "$indices" ]; then
  printf '%s\n' 'nvidia-smi returned no GPUs' >&2
  exit 1
fi
for gpu in $indices; do
  "$nvidia_smi" --id="$gpu" --power-limit={watts} >/dev/null
done
"$nvidia_smi" \
  --query-gpu=index,power.limit,power.min_limit,power.max_limit \
  --format=csv,noheader,nounits
"#
    );
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: registry now requires {watts} W at generation {generation}, but immediate reconciliation failed: {}",
            crate::deploy::host_channel::last_error_line(
                &output,
                "remote nvidia-smi power-limit update failed"
            )
        )));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "target": target,
                "gpu_power_limit_watts": watts,
                "registry_generation": generation,
                "driver": output.stdout.trim(),
                "status": "reconciled",
            }))?
        );
    } else {
        println!("{target}: gpu_power_limit_watts={watts} (generation {generation})");
        print!("{}", output.stdout);
    }
    Ok(())
}

/// `stado host cron TARGET [--prune TEXT] [--apply] [--restore PATH]` — the
/// periodic table, and the one sanctioned way to change it.
///
/// A crontab is the last place on a fleet host where a process can be
/// declared outside both launchd and the registry, which is why every repair
/// this product makes to a unit can be undone by a reboot. See
/// [`crate::deploy::host_cron`] for the guards; they are on the host.
pub async fn cron(
    target: &str,
    prune: Option<&str>,
    restore: Option<&str>,
    apply: bool,
    json: bool,
) -> Result<(), CmdError> {
    if apply && prune.is_none() {
        return Err(CmdError::usage(
            "--apply changes a table, so it needs --prune to say which line",
        ));
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let outcome = match restore {
        Some(path) => crate::deploy::host_cron::restore(&resolved, path, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?,
        None => crate::deploy::host_cron::prune(&resolved, prune.unwrap_or(""), apply, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome.to_json())?);
    } else {
        println!("\n{}: crontab {}", outcome.host, outcome.state);
        if !outcome.detail.is_empty() {
            println!("  {}", outcome.detail);
        }
        if !outcome.table.is_empty() {
            println!("\nthe table as this host has it:");
            for row in &outcome.table {
                println!("  {row}");
            }
        }
        if !outcome.matched.is_empty() {
            println!("\nwhat the pattern reached:");
            for row in &outcome.matched {
                println!("  {row}");
            }
        }
        // Printed with the change, never left for the operator to compose.
        if let Some(command) = outcome.restore_command() {
            println!("\nthe table it replaced is saved; this puts it back:\n  {command}");
        }
    }
    if outcome.succeeded() {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "{}: crontab {} — {}",
            outcome.host, outcome.state, outcome.detail
        )))
    }
}
