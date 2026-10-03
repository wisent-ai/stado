use crate::common::sha256;
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
    let manifest: Value = serde_json::from_slice(
        &fs::read(manifest).with_context(|| format!("reading {}", manifest.display()))?,
    )?;
    let product = manifest["product"]
        .as_str()
        .context("release manifest has no product")?;
    let root = output
        .canonicalize()
        .with_context(|| format!("the build output {} is missing", output.display()))?;
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
        let resolved = source.canonicalize().with_context(|| {
            format!(
                "the build did not produce stage key {} inside WISENT_OUTPUT_DIR {}",
                source.strip_prefix(&root).unwrap_or(&source).display(),
                root.display()
            )
        })?;
        if !resolved.starts_with(&root) {
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
    let signed: Vec<PathBuf> = selected.keys().cloned().collect();
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
    let reports = match (result, cleanup) {
        (Ok(reports), Ok(())) => reports,
        (Err(error), Err(cleanup)) => {
            return Err(error.context(format!(
                "signing credential cleanup also failed: {cleanup:#}"
            )))
        }
        (Err(error), Ok(())) | (Ok(_), Err(error)) => return Err(error),
    };
    refresh_checksums(&root, stage.keys().map(String::as_str), &signed)?;
    Ok(reports)
}

/// Signing rewrites a native file in place, so a `SHA256SUMS` the build wrote
/// beside it names bytes that no longer exist, and whatever later checks the
/// released file against it refuses the release it belongs to. Each staged
/// `SHA256SUMS` has the line of every file signed here, in its own directory,
/// rewritten with the digest of the signed bytes; every other line is kept.
fn refresh_checksums<'a>(
    root: &Path,
    sources: impl Iterator<Item = &'a str>,
    signed: &[PathBuf],
) -> Result<()> {
    for source in sources {
        let path = root.join(source);
        if path.file_name().and_then(|name| name.to_str()) != Some("SHA256SUMS") || !path.is_file()
        {
            continue;
        }
        let directory = path.parent().context("checksum file has no directory")?;
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut lines = Vec::new();
        for line in text.lines() {
            let Some((_, name)) = line.split_once(char::is_whitespace) else {
                lines.push(line.to_owned());
                continue;
            };
            let name = name.trim_start().trim_start_matches('*');
            let member = directory.join(name);
            if signed.iter().any(|file| file == &member) {
                lines.push(format!("{}  {name}", sha256(&member)?));
            } else {
                lines.push(line.to_owned());
            }
        }
        let mut rewritten = lines.join("\n");
        rewritten.push('\n');
        fs::write(&path, rewritten).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}
