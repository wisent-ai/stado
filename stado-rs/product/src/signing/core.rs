use super::constants::{DEVELOPER_ID, DEVELOPMENT, MACH_O_MAGICS};
use crate::common::{capture, checked};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output},
};

pub fn command(program: &str, args: &[&str], require_success: bool) -> Result<Output> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(unix)]
    unsafe {
        let account = libc::getpwuid(libc::geteuid());
        if account.is_null() || (*account).pw_dir.is_null() {
            bail!("cannot resolve the effective account's keychain home");
        }
        command.env(
            "HOME",
            std::ffi::CStr::from_ptr((*account).pw_dir)
                .to_string_lossy()
                .as_ref(),
        );
    }
    if require_success {
        checked(&mut command)
    } else {
        capture(&mut command)
    }
}

pub fn native(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut bytes = [0; 4];
    let count = File::open(path)?.read(&mut bytes)?;
    Ok(count == bytes.len() && MACH_O_MAGICS.contains(&bytes))
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    let expanded = match path.to_str().and_then(|p| p.strip_prefix("~/")) {
        Some(suffix) => {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(suffix)
        }
        None => path.to_path_buf(),
    };
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        Ok(std::env::current_dir()?.join(expanded))
    }
}

pub fn identifier(product: &str, name: &str) -> Result<String> {
    let identifier = format!("ai.wisent.{product}.{name}");
    validate_identifier(&identifier)?;
    Ok(identifier)
}

pub fn validate_identifier(identifier: &str) -> Result<()> {
    if identifier.is_empty()
        || !identifier.as_bytes()[0].is_ascii_alphanumeric()
        || !identifier
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        bail!("invalid code identifier: {identifier}");
    }
    Ok(())
}

pub fn bundle(path: &Path) -> bool {
    path.is_dir()
        && matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("app" | "framework" | "xpc" | "appex")
        )
}

pub fn inspect(path: &Path) -> Result<Value> {
    let path = absolute(path)?;
    let mut report = json!({"path": path, "state": "not_native", "identifier": null,
        "team": null, "authority": null, "requirement": null, "error": null, "hardened_runtime": null});
    if !path.exists() {
        return failed(
            report,
            "missing",
            &format!("signing target does not exist: {}", path.display()),
        );
    }
    if !cfg!(target_os = "macos") {
        report["state"] = json!("not_applicable");
        return Ok(report);
    }
    if !native(&path)? && !bundle(&path) {
        return Ok(report);
    }
    let pathname = path.to_str().context("signing path is not UTF-8")?;
    let shown = command(
        "/usr/bin/codesign",
        &["--display", "--verbose=4", "-r-", pathname],
        false,
    )?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&shown.stdout),
        String::from_utf8_lossy(&shown.stderr)
    );
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        if let Some((field, value)) = line.split_once('=') {
            fields.entry(field).or_insert(value);
        }
        if let Some((_, value)) = line.split_once("designated => ") {
            report["requirement"] = json!(value);
        }
        if let Some(flags) = line.strip_prefix("CodeDirectory ").and_then(|value| {
            value
                .split_whitespace()
                .find(|word| word.starts_with("flags="))
        }) {
            report["hardened_runtime"] =
                json!(flags.split_once('(').is_some_and(|(_, names)| names
                    .trim_end_matches(')')
                    .split(',')
                    .any(|name| name == "runtime")));
        }
    }
    report["identifier"] = json!(fields.get("Identifier"));
    report["team"] = json!(fields.get("TeamIdentifier"));
    report["authority"] = json!(fields.get("Authority"));
    report["cdhash"] = json!(fields.get("CDHash"));
    if !shown.status.success() {
        return failed(report, "unsigned", text.trim());
    }
    let verified = command(
        "/usr/bin/codesign",
        &["--verify", "--strict", "--all-architectures", pathname],
        false,
    )?;
    if !verified.status.success() {
        return failed(
            report,
            "invalid",
            String::from_utf8_lossy(&verified.stderr).trim(),
        );
    }
    if fields.get("Signature") == Some(&"adhoc")
        || report["requirement"]
            .as_str()
            .is_some_and(|r| r.contains("cdhash "))
    {
        return failed(
            report,
            "adhoc",
            "code identity is tied to this build, not an Apple signing identity",
        );
    }
    let authority = report["authority"].as_str().unwrap_or("");
    if !(authority.starts_with(DEVELOPER_ID) || authority.starts_with(DEVELOPMENT))
        || report["team"].is_null()
        || report["team"] == "not set"
    {
        return failed(
            report,
            "untrusted",
            "code has no Apple Development or Developer ID Application identity",
        );
    }
    let anchored = command(
        "/usr/bin/codesign",
        &[
            "--verify",
            "--strict",
            "-R",
            "=anchor apple generic",
            pathname,
        ],
        false,
    )?;
    if !anchored.status.success() {
        return failed(
            report,
            "untrusted",
            String::from_utf8_lossy(&anchored.stderr).trim(),
        );
    }
    report["state"] = json!("stable");
    Ok(report)
}

fn failed(mut report: Value, state: &str, error: &str) -> Result<Value> {
    report["state"] = json!(state);
    report["error"] = json!(error);
    Ok(report)
}

pub fn compatible(path: &Path, previous: &Value) -> Result<()> {
    if previous["state"] != "stable" {
        return Ok(());
    }
    let requirement = format!(
        "={}",
        previous["requirement"]
            .as_str()
            .context("stable identity has no designated requirement")?
    );
    command(
        "/usr/bin/codesign",
        &[
            "--verify",
            "--strict",
            "-R",
            &requirement,
            path.to_str().context("non-UTF8 signing target")?,
        ],
        true,
    )?;
    Ok(())
}
