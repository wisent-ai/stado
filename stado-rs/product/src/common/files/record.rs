use super::super::process::birth::started_at;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    path::Path,
};

pub(crate) struct Recorded {
    pub product: Option<String>,
    pub pid: i32,
    pub started_at: Option<String>,
    pub command: String,
    pub acquired_at: String,
    pub phase: String,
    pub step: Option<String>,
}

pub(super) fn product(path: &Path) -> Result<&str> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .with_context(|| format!("{} has no product directory", path.display()))
}

pub(crate) fn read(path: &Path, expected: Option<&str>) -> Result<Option<Recorded>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading lock record {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(text.trim())
        .with_context(|| format!("{} records invalid JSON", path.display()))?;
    let command = value["command"]
        .as_str()
        .with_context(|| format!("{} records no command", path.display()))?;
    // Older writers did not record the guarded product. The path, not the
    // command line, names that product; a dependency shares its caller's argv.
    let named = value["product"].as_str().or(expected);
    let pid = value["pid"]
        .as_u64()
        .with_context(|| format!("{} records no pid", path.display()))?;
    let phase = match value["phase"].as_str() {
        Some(phase) => phase.to_owned(),
        None => super::locks::PHASE_PLACING.to_owned(),
    };
    Ok(Some(Recorded {
        product: named.map(str::to_owned),
        pid: i32::try_from(pid)
            .with_context(|| format!("{} records an invalid pid {pid}", path.display()))?,
        started_at: value["started_at"].as_str().map(str::to_owned),
        command: command.to_owned(),
        acquired_at: value["acquired_at"]
            .as_str()
            .with_context(|| format!("{} records no acquisition time", path.display()))?
            .to_owned(),
        phase,
        step: value["step"].as_str().map(str::to_owned),
    }))
}

impl Recorded {
    pub fn validate_product(&self, path: &Path, expected: &str) -> Result<()> {
        if let Some(named) = &self.product {
            if named != expected {
                bail!(
                    "{} guards product {expected} but records product {named}; held lock is corrupt",
                    path.display()
                );
            }
        }
        Ok(())
    }

    pub fn observed_start(&self) -> Result<Option<String>> {
        started_at(self.pid)
    }

    pub fn verified_live(&self) -> Result<bool> {
        match &self.started_at {
            Some(expected) => Ok(self.observed_start()?.as_ref() == Some(expected)),
            None => Ok(false),
        }
    }
}

/// Read every surface lock without treating a free file's stale record as a
/// live installation. Corrupt records stay visible with their actual cause.
pub fn locks_report(home: &Path) -> Result<Value> {
    let directory = home.join(".stado/products");
    let products = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"locks": []}))
        }
        Err(error) => {
            return Err(error).with_context(|| format!("listing {}", directory.display()))
        }
    };
    let mut rows = Vec::new();
    for product_entry in products {
        let product_entry = product_entry?;
        if !product_entry.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(product_entry.path())? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("lock") {
                continue;
            }
            let mut row = json!({"path": path, "product": product(&path)?,
                "surface": path.file_stem().and_then(|name| name.to_str())});
            let inspected = (|| -> Result<Value> {
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW);
                }
                let file = options.open(&path)?;
                let free = match file.try_lock_shared() {
                    Ok(()) => {
                        file.unlock()?;
                        true
                    }
                    Err(std::fs::TryLockError::WouldBlock) => false,
                    Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
                };
                let recorded = read(&path, Some(product(&path)?))?;
                let Some(recorded) = recorded else {
                    return Ok(json!({"held": !free, "error": "lock records no holder"}));
                };
                if !free {
                    recorded.validate_product(&path, product(&path)?)?;
                }
                let observed = recorded.observed_start()?;
                let running = recorded
                    .started_at
                    .as_ref()
                    .is_some_and(|start| observed.as_ref() == Some(start));
                Ok(json!({"held": !free, "pid": recorded.pid,
                    "recorded_product": recorded.product, "command": recorded.command,
                    "phase": recorded.phase, "acquired_at": recorded.acquired_at,
                    "started_at": recorded.started_at, "observed_start": observed,
                    "running": running}))
            })();
            match inspected {
                Ok(Value::Object(fields)) => row
                    .as_object_mut()
                    .context("lock row is not an object")?
                    .extend(fields),
                Ok(_) => bail!("lock inspection did not produce fields"),
                Err(error) => row["error"] = json!(format!("{error:#}")),
            }
            rows.push(row);
        }
    }
    rows.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    Ok(json!({"locks": rows}))
}
