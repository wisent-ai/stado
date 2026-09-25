use crate::signing::{
    core::{identifier, inspect, native},
    signer::Signer,
    Policy,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

fn files(path: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    if path.symlink_metadata()?.file_type().is_symlink() {
        bail!("release signing input is a symlink: {}", path.display());
    }
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            files(&entry?.path(), output)?;
        }
    } else if native(path)? {
        output.push(path.to_path_buf());
    }
    Ok(())
}

pub fn stage(manifest: &Path, output: &Path, platform: &str) -> Result<Vec<Value>> {
    let manifest: Value = serde_json::from_slice(&fs::read(manifest)?)?;
    let product = manifest["product"]
        .as_str()
        .context("release manifest has no product")?;
    let root = output.canonicalize()?;
    let stage = manifest["platforms"][platform]["stage"]
        .as_object()
        .context("release platform has no stage map")?;
    let mut selected = BTreeMap::new();
    for (source, destination) in stage {
        let source = Path::new(source);
        let destination = Path::new(
            destination
                .as_str()
                .context("stage destination must be a path")?,
        );
        if source.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) || destination.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            bail!("release signing path escapes its root");
        }
        let source = root.join(source);
        if !source.canonicalize()?.starts_with(&root) {
            bail!("release signing input escapes output: {}", source.display());
        }
        let mut members = Vec::new();
        files(&source, &mut members)?;
        for member in members {
            let archive_path = if source.is_dir() {
                destination.join(member.strip_prefix(&source)?)
            } else {
                destination.to_path_buf()
            };
            let code_id = identifier(
                product,
                archive_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .context("stage filename missing")?,
            )?;
            if let Some(previous) = selected.insert(member.clone(), code_id.clone()) {
                if previous != code_id {
                    bail!(
                        "one native input has conflicting release identities: {}",
                        member.display()
                    );
                }
            }
        }
    }
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let mut signer = Signer::new(&root, None)?;
    let result = (|| {
        let mut reports = Vec::new();
        for (path, identifier) in selected {
            let report = inspect(&path)?;
            let policy = Policy::default();
            let code_id = if report["state"] == "stable" {
                report["identifier"]
                    .as_str()
                    .context("stable release input has no identifier")?
            } else {
                &identifier
            };
            reports.push(
                if report["state"] == "stable" && signer.preserves_existing(&policy) {
                    report
                } else {
                    signer.sign(&path, code_id, None, &policy)?
                },
            );
        }
        Ok(reports)
    })();
    let cleanup = signer.close();
    match (result, cleanup) {
        (Ok(reports), Ok(())) => Ok(reports),
        (Err(error), Err(cleanup)) => Err(error.context(format!(
            "signing credential cleanup also failed: {cleanup:#}"
        ))),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}
