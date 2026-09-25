use crate::{catalog::text, common::relative};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn inside(root: &Path, name: &str) -> Result<PathBuf> {
    let path = root.join(relative(Path::new(name))?);
    if !path.canonicalize()?.starts_with(root.canonicalize()?) {
        bail!("declared path escapes {}: {name}", root.display());
    }
    Ok(path)
}

pub fn load(root: &Path, name: &str) -> Result<Value> {
    let path = inside(root, name)?;
    serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("reading release manifest {}", path.display()))
}

pub fn version(root: &Path, document: &Value) -> Result<String> {
    let source = &document["version_source"];
    if source["kind"] != "regex" {
        bail!("release manifest has no supported regex version_source");
    }
    let path = inside(root, text(source, "path")?)?;
    let pattern = regex::Regex::new(text(source, "pattern")?)?;
    let content = fs::read_to_string(&path)?;
    let version = pattern
        .captures(&content)
        .and_then(|m| m.name("version"))
        .map(|v| v.as_str().to_owned())
        .with_context(|| {
            format!(
                "release version pattern did not capture version in {}",
                path.display()
            )
        })?;
    if version.is_empty() {
        bail!(
            "release version pattern returned an empty version in {}",
            path.display()
        );
    }
    Ok(version)
}

pub fn secrets(sources: &[&Value]) -> Result<BTreeMap<String, String>> {
    let mut declared = serde_json::Map::new();
    for source in sources {
        if let Some(secrets) = source.get("secret_env") {
            declared.extend(
                secrets
                    .as_object()
                    .context("secret_env must be an object")?
                    .clone(),
            );
        }
    }
    let mut resolved = BTreeMap::new();
    for (name, coordinate) in declared {
        if name.is_empty()
            || name.as_bytes()[0].is_ascii_digit()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            bail!("invalid build secret environment name {name}");
        }
        let coordinate = coordinate
            .as_str()
            .context("build secret coordinate must be item#field")?;
        let (item, field) = coordinate
            .split_once('#')
            .filter(|(item, field)| !item.is_empty() && !field.is_empty())
            .with_context(|| format!("{name} <- {coordinate}: expected item#field"))?;
        // Secret bytes are carried only in memory and the child's environment, never command logs or arguments.
        let output = Command::new("skarbiec")
            .args(["get", item, "--field", field])
            .output()
            .with_context(|| format!("resolving {name} from {coordinate}"))?;
        if !output.status.success() {
            eprintln!(
                "build secret unavailable: {name} <- {coordinate}: {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            continue;
        }
        let value = String::from_utf8(output.stdout)?.trim().to_owned();
        if value.is_empty() {
            eprintln!("build secret unavailable: {name} <- {coordinate}: no value");
        } else {
            resolved.insert(name, value);
        }
    }
    Ok(resolved)
}
