mod definition;
use crate::common::{atomic_write, checked, emit, Arguments, Runtime};
use anyhow::{bail, Context, Result};
use definition::Surface;
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, process::Command};

fn loaded() -> Result<BTreeMap<String, Value>> {
    let output = checked(Command::new("launchctl").arg("list"))?;
    let mut rows = BTreeMap::new();
    for line in String::from_utf8(output.stdout)?.lines().skip(1) {
        let columns: Vec<_> = line.split_whitespace().collect();
        if columns.len() != 3 {
            bail!("launchctl list returned an unrecognized row: {line}");
        }
        rows.insert(
            columns[2].to_owned(),
            json!({"loaded": "yes", "PID": columns[0], "LastExitStatus": columns[1]}),
        );
    }
    Ok(rows)
}

fn update(runtime: &Runtime, surface: Surface, remove: bool) -> Result<Value> {
    let path = surface.path(runtime);
    let before = loaded()?;
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let target = format!("{domain}/{}", surface.label());
    if remove {
        if before.contains_key(surface.label()) {
            checked(Command::new("launchctl").args(["bootout", &target]))?;
        }
        match fs::remove_file(&path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        if loaded()?.contains_key(surface.label()) || path.exists() {
            bail!("{} is still installed after removal", surface.label());
        }
        return Ok(
            json!({"surface": surface.name(), "label": surface.label(), "removed": "yes", "loaded": "no"}),
        );
    }
    definition::executable(runtime)?;
    let desired = definition::render(runtime, surface)?;
    let previous = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if previous.as_deref() == Some(desired.as_bytes()) && before.contains_key(surface.label()) {
        return status(runtime, surface, &before);
    }
    let evidence = runtime
        .output
        .join("schedule")
        .join(uuid::Uuid::new_v4().to_string());
    let candidate = evidence.join("candidate.plist");
    atomic_write(&candidate, desired.as_bytes())?;
    checked(Command::new("plutil").arg("-lint").arg(&candidate))?;
    if let Some(previous) = &previous {
        atomic_write(&evidence.join("previous.plist"), previous)?;
    }
    fs::create_dir_all(runtime.home.join("Library/Logs/wisent"))?;
    if before.contains_key(surface.label()) {
        checked(Command::new("launchctl").args(["bootout", &target]))?;
    }
    let result = (|| {
        atomic_write(&path, desired.as_bytes())?;
        checked(
            Command::new("launchctl")
                .args(["bootstrap", &domain])
                .arg(&path),
        )?;
        let observed = loaded()?;
        if !observed.contains_key(surface.label()) {
            bail!("launchd did not retain {} after bootstrap", surface.label());
        }
        status(runtime, surface, &observed)
    })();
    match result {
        Ok(row) => Ok(row),
        Err(error) => {
            let restore = (|| -> Result<()> {
                if loaded()?.contains_key(surface.label()) {
                    checked(Command::new("launchctl").args(["bootout", &target]))?;
                }
                if let Some(previous) = previous {
                    atomic_write(&path, &previous)?;
                    if before.contains_key(surface.label()) {
                        checked(
                            Command::new("launchctl")
                                .args(["bootstrap", &domain])
                                .arg(&path),
                        )?;
                    }
                } else if path.exists() {
                    fs::remove_file(&path)?;
                }
                Ok(())
            })();
            if let Err(restore) = restore {
                bail!("schedule update failed: {error:#}; restoring the previous agent also failed: {restore:#}; evidence: {}", evidence.display());
            }
            Err(error).with_context(|| {
                format!(
                    "previous scheduler state restored; evidence: {}",
                    evidence.display()
                )
            })
        }
    }
}

fn status(
    runtime: &Runtime,
    surface: Surface,
    observed: &BTreeMap<String, Value>,
) -> Result<Value> {
    let path = surface.path(runtime);
    let expected = definition::render(runtime, surface)?;
    let actual = match fs::read_to_string(&path) {
        Ok(actual) => Some(actual),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut row = observed
        .get(surface.label())
        .cloned()
        .unwrap_or_else(|| json!({"loaded": "no"}));
    row["surface"] = json!(surface.name());
    row["label"] = json!(surface.label());
    row["interval"] = json!(definition::INTERVAL_SECONDS.to_string());
    row["plist"] = json!(match actual.as_deref() {
        None => "absent",
        Some(text) if text == expected => "current",
        Some(_) => "differs from this version",
    });
    row["path"] = json!(path);
    Ok(row)
}

pub fn run(arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(arguments);
    if !args.positional.is_empty() || (args.has("--install") && args.has("--remove")) {
        bail!("schedule accepts either --install or --remove, not both");
    }
    if !cfg!(target_os = "macos") {
        bail!("scheduled agents require macOS launchd; use this host's scheduler for stado product sync --surface cli --fetch");
    }
    let mut reports = Vec::new();
    let observed = loaded()?;
    let mut failed = false;
    for surface in [Surface::Cli, Surface::Desktop] {
        let result = if args.has("--install") || args.has("--remove") {
            update(runtime, surface, args.has("--remove"))
        } else {
            status(runtime, surface, &observed)
        };
        match result {
            Ok(row) => reports.push(row),
            Err(error) => {
                failed = true;
                reports.push(json!({"surface": surface.name(), "label": surface.label(), "error": format!("{error:#}")}));
            }
        }
    }
    emit(&json!(reports))?;
    Ok(i32::from(failed))
}
