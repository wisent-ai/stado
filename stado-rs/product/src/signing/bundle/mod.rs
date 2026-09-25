use super::{
    core::{bundle, command, compatible, inspect, native, validate_identifier},
    signer::Signer,
    Policy,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn identifier(path: &Path) -> Result<String> {
    for relative in ["Contents/Info.plist", "Resources/Info.plist"] {
        let info = path.join(relative);
        if !info.is_file() {
            continue;
        }
        let output = command(
            "/usr/bin/plutil",
            &[
                "-extract",
                "CFBundleIdentifier",
                "raw",
                "-o",
                "-",
                info.to_str().context("non-UTF8 plist path")?,
            ],
            true,
        )?;
        let identifier = String::from_utf8(output.stdout)?.trim().to_owned();
        validate_identifier(&identifier)?;
        return Ok(identifier);
    }
    bail!(
        "application member has no valid CFBundleIdentifier: {}",
        path.display()
    )
}

fn members(path: &Path, files: &mut Vec<PathBuf>, bundles: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            if bundle(&path) {
                bundles.push(path.clone());
            }
            members(&path, files, bundles)?;
        } else if kind.is_file() && native(&path)? {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn exchange(staged: &Path, destination: &Path) -> Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(staged.as_os_str().as_bytes())?;
    let target = CString::new(destination.as_os_str().as_bytes())?;
    if unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_SWAP) } != 0 {
        return Err(std::io::Error::last_os_error()).context("atomically replacing signed bundle");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn exchange(_staged: &Path, _destination: &Path) -> Result<()> {
    bail!("Apple bundle signing requires a Darwin host")
}

pub fn sign(
    signer: &Signer,
    path: &Path,
    code_identifier: &str,
    previous: Option<&Path>,
    policy: &Policy,
) -> Result<Value> {
    validate_identifier(code_identifier)?;
    if !bundle(path) {
        bail!(
            "signing target is not a supported application bundle: {}",
            path.display()
        );
    }
    let observed = inspect(path)?;
    let before = inspect(previous.filter(|p| p.exists()).unwrap_or(path))?;
    if before["state"] == "stable" && before["identifier"] != code_identifier {
        bail!(
            "bundle identifier would change: {} -> {code_identifier}",
            before["identifier"]
        );
    }
    if observed["state"] == "stable" && signer.preserves_existing(policy) {
        command(
            "/usr/bin/codesign",
            &[
                "--verify",
                "--strict",
                "--deep",
                path.to_str().context("non-UTF8 bundle path")?,
            ],
            true,
        )?;
        compatible(path, &before)?;
        return Ok(observed);
    }
    let identity = signer.identity(&before)?;
    let directory = path
        .parent()
        .context("bundle has no parent")?
        .join(format!(".wisent-signing-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&directory)?;
    let staged = directory.join(path.file_name().context("bundle has no filename")?);
    let result = (|| {
        command(
            "/usr/bin/ditto",
            &[
                path.to_str().context("non-UTF8 bundle path")?,
                staged.to_str().context("non-UTF8 stage path")?,
            ],
            true,
        )?;
        let mut files = Vec::new();
        let mut bundles = Vec::new();
        members(&staged, &mut files, &mut bundles)?;
        for member in files {
            if inspect(&member)?["state"] == "stable" {
                continue;
            }
            let relative: String = member
                .strip_prefix(&staged)?
                .to_string_lossy()
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || "._-".contains(c) {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            signer.stable_sign(
                &member,
                &identity,
                &format!("{code_identifier}.{relative}"),
                &Policy::default(),
            )?;
        }
        bundles.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for bundle in bundles {
            signer.stable_sign(
                &bundle,
                &identity,
                &identifier(&bundle)?,
                &Policy::default(),
            )?;
        }
        let mut final_report = signer.stable_sign(&staged, &identity, code_identifier, policy)?;
        command(
            "/usr/bin/codesign",
            &[
                "--verify",
                "--strict",
                "--deep",
                staged.to_str().context("non-UTF8 stage path")?,
            ],
            true,
        )?;
        compatible(&staged, &before)?;
        exchange(&staged, path)?;
        final_report["path"] = json!(path);
        final_report["previous_state"] = before["state"].clone();
        Ok(final_report)
    })();
    let cleanup = fs::remove_dir_all(&directory);
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error).context("removing retained bundle signing scratch"),
    }
}
