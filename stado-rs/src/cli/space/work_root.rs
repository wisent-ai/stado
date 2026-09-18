//! `stado space work-root TARGET [--path PATH]`: where this host keeps the
//! fleet's work, read from the registry or declared into it.
//!
//! A declaration is two writes that have to agree: the directory on the
//! host, owned by the account the agent's unit runs as, and the
//! `targets[].work_root` field the agent reads it from. The host is written
//! first — a registry that names a directory the agent cannot open would
//! close the queue on that host at its next restart — and the registry only
//! after the host reports the directory is there, owned, and on a mounted
//! device-backed filesystem that is not the root volume the declaration
//! exists to leave.

use serde_json::{json, Value};

use crate::cli::space::print_json;
use crate::cli::CmdError;
use crate::deploy::{host_channel, shlex_quote};
use crate::primitives::failure::FailureCode;
use crate::providers::local::work_base;

/// Substitution points in [`PROGRAM`]; each is shell-quoted before splicing.
const PATH_MARK: &str = "@PATH@";
const UNIT_MARK: &str = "@UNIT@";

/// The agent unit whose `User=` owns the work root. One name across the
/// fleet: [`crate::deploy::bootstrap`] writes it on every local host.
const AGENT_UNIT: &str = "wisent-compute-agent.service";

/// The fixed host program: find the agent's account, make the directory,
/// hand it over, and say which filesystem it landed on.
const PROGRAM: &str = r#"set -u
path=@PATH@
unit=@UNIT@
user=""
if command -v systemctl >/dev/null 2>&1; then
  user=$(systemctl show -p User --value "$unit" 2>/dev/null || true)
fi
if [ -z "$user" ]; then
  user=$(id -un)
fi
if ! id -u "$user" >/dev/null 2>&1; then
  printf 'ERROR\tthe agent unit %s names account %s, which this host does not have\n' "$unit" "$user"
  exit 1
fi
if [ -e "$path" ] && [ ! -d "$path" ]; then
  printf 'ERROR\t%s exists and is not a directory\n' "$path"
  exit 1
fi
mkdir -p "$path" || { printf 'ERROR\tcannot create %s\n' "$path"; exit 1; }
chown "$user" "$path" || { printf 'ERROR\tcannot make %s the owner of %s\n' "$user" "$path"; exit 1; }
chmod 0700 "$path" || { printf 'ERROR\tcannot restrict %s\n' "$path"; exit 1; }
device=$(/bin/df -P "$path" 2>/dev/null | awk 'NR == 2 { print $1 }')
root_device=$(/bin/df -P / 2>/dev/null | awk 'NR == 2 { print $1 }')
free_kb=$(/bin/df -Pk "$path" 2>/dev/null | awk 'NR == 2 { print $4 }')
mounted_on=$(/bin/df -P "$path" 2>/dev/null | awk 'NR == 2 { print $6 }')
case "$device" in
  /dev/*) ;;
  *) printf 'ERROR\t%s is on %s, which is not a device-backed filesystem\n' "$path" "${device:-nothing}"; exit 1 ;;
esac
if [ "$device" = "$root_device" ]; then
  printf 'ERROR\t%s is on the root volume %s; a work root exists to leave it, so declare a directory on another mounted disk (`stado space report` lists them)\n' "$path" "$device"
  exit 1
fi
printf 'STADO_WORK_ROOT\t%s\t%s\t%s\t%s\t%s\n' "$path" "$user" "$device" "$mounted_on" "${free_kb:-}"
"#;

/// What the host said after making the directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Prepared {
    path: String,
    owner: String,
    device: String,
    mounted_on: String,
    free_kb: String,
    error: Option<String>,
}

fn parse(stdout: &str) -> Prepared {
    let mut prepared = Prepared::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_WORK_ROOT", path, owner, device, mounted_on, free_kb] => {
                prepared.path = (*path).to_string();
                prepared.owner = (*owner).to_string();
                prepared.device = (*device).to_string();
                prepared.mounted_on = (*mounted_on).to_string();
                prepared.free_kb = (*free_kb).to_string();
            }
            ["ERROR", message] => prepared.error = Some((*message).to_string()),
            _ => {}
        }
    }
    prepared
}

/// `space work-root` body.
pub async fn dispatch(target: &str, path: Option<&str>, json: bool) -> Result<(), CmdError> {
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
    let declared = entry
        .get("work_root")
        .and_then(Value::as_str)
        .map(str::to_string);

    let Some(path) = path else {
        if json {
            return print_json(&json!({
                "target": target,
                "work_root": declared,
                "jobs": declared.as_deref().map(|root| format!("{root}/{}", work_base::JOBS_LEAF)),
                "build_cache": declared.as_deref().map(|root| format!("{root}/{}", work_base::BUILD_CACHE_LEAF)),
            }));
        }
        match declared {
            Some(root) => println!(
                "{target}: work root {root} (job trees under {root}/{}, build caches under {root}/{})",
                work_base::JOBS_LEAF,
                work_base::BUILD_CACHE_LEAF
            ),
            None => println!(
                "{target}: no work root declared; the agent keeps job trees under ~/{} and build caches under ~/.stado/{}",
                crate::providers::local::disk_cleanup::queue_workdirs::WORK_ROOT,
                work_base::BUILD_CACHE_LEAF
            ),
        }
        return Ok(());
    };

    if let Err(problem) = work_base::validate_declared(path) {
        return Err(CmdError::usage(format!("--path {problem}"))
            .stating(FailureCode::Refused)
            .machine_readable(json));
    }
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()).machine_readable(json))?;
    let runner = crate::deploy::production_runner();
    let program = PROGRAM
        .replace(PATH_MARK, &shlex_quote(path))
        .replace(UNIT_MARK, &shlex_quote(AGENT_UNIT));
    let output = host_channel::run_script(&resolved, &program, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()).machine_readable(json))?;
    let prepared = parse(&output.stdout);
    if let Some(error) = prepared.error {
        return Err(CmdError::click(format!("{target}: {error}"))
            .stating(FailureCode::Refused)
            .machine_readable(json));
    }
    if prepared.path.is_empty() {
        return Err(CmdError::click(format!(
            "{target}: the host program exited {} without preparing {path}; stdout: {}",
            output.code,
            output.stdout.trim()
        ))
        .machine_readable(json));
    }

    entry.insert("work_root".to_string(), Value::String(path.to_string()));
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let generation = store.compare_and_swap(&current.version, &payload).await?;
    if json {
        return print_json(&json!({
            "target": target,
            "generation": generation,
            "work_root": path,
            "owner": prepared.owner,
            "device": prepared.device,
            "mounted_on": prepared.mounted_on,
            "free_kb": prepared.free_kb,
            "previous": declared,
            "takes_effect": "when the agent's unit next starts it",
        }));
    }
    println!(
        "{target}: work root {path} written at generation {generation}; the directory is owned by {} on {} mounted at {} with {} free KiB. The agent uses it once its unit restarts it",
        prepared.owner, prepared.device, prepared.mounted_on, prepared.free_kb
    );
    Ok(())
}
