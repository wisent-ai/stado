use super::credentials::Credentials;
use crate::support::{command, Run};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use stado_product::common::sha256;
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
};

// Apple's CS_RUNTIME flag in kern/cs_blobs.h, not a configurable threshold.
const HARDENED_RUNTIME_FLAG: u32 = 0x0001_0000;
const RUNNER_ENTITLEMENTS: &[&str] = &[
    "com.apple.security.cs.allow-jit",
    "com.apple.security.cs.allow-unsigned-executable-memory",
    "com.apple.security.cs.allow-dyld-environment-variables",
    "com.apple.security.cs.disable-library-validation",
];

pub fn entitlements(run: &mut Run, path: &Path) -> Result<plist::Value> {
    let mut inspect = Command::new("/usr/bin/codesign");
    inspect
        .args(["--display", "--entitlements", "-", "--xml"])
        .arg(path);
    let observed = command(run, inspect)?;
    observed.passed()?;
    let bytes = fs::read(observed.directory.join("stdout.log"))?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(plist::Value::Dictionary(plist::Dictionary::new()));
    }
    Ok(plist::Value::from_reader(Cursor::new(bytes))?)
}

pub fn requested(run: &mut Run, path: &Path, label: &str) -> Result<(PathBuf, plist::Value)> {
    let mut requested = entitlements(run, path)?;
    let dictionary = requested
        .as_dictionary_mut()
        .context("actual entitlement dictionary")?;
    for key in RUNNER_ENTITLEMENTS {
        dictionary.insert((*key).into(), plist::Value::Boolean(true));
    }
    let file = run.root.join(format!("{label}-entitlements.plist"));
    requested.to_file_xml(&file)?;
    Ok((file, requested))
}

pub fn inspect(run: &mut Run, path: &Path) -> Result<Value> {
    let cmd = run.product(&[
        "signing",
        "inspect",
        path.to_str().context("non-UTF8 test path")?,
        "--json",
    ]);
    let mut reports = command(run, cmd)?.json()?;
    reports
        .as_array_mut()
        .context("actual signature reports")?
        .pop()
        .context("signature report is absent")
}

pub fn sign(
    run: &mut Run,
    credentials: &Credentials,
    path: &Path,
    identifier: &str,
    previous: Option<&Path>,
    entitlements: &Path,
) -> Result<Value> {
    let mut cmd = run.product(&[
        "signing",
        "sign",
        path.to_str().context("non-UTF8 test path")?,
        "--identifier",
        identifier,
        "--hardened-runtime",
        "--entitlements",
        entitlements.to_str().context("non-UTF8 entitlement path")?,
        "--json",
    ]);
    if let Some(previous) = previous {
        cmd.arg("--previous").arg(previous);
    }
    credentials.configure(&mut cmd);
    let mut reports = command(run, cmd)?.json()?;
    let report = reports
        .as_array_mut()
        .context("actual signature reports")?
        .pop()
        .context("signature report is absent")?;
    ensure!(
        report["state"] == "stable",
        "the signer did not produce a stable identity: {report}"
    );
    Ok(report)
}

pub fn verify(
    run: &mut Run,
    path: &Path,
    preceding: &Value,
    expected: &plist::Value,
) -> Result<()> {
    let requirement = format!(
        "={}",
        preceding["requirement"]
            .as_str()
            .context("preceding designated requirement")?
    );
    let mut verify = Command::new("/usr/bin/codesign");
    verify
        .args([
            "--verify",
            "--strict",
            "--all-architectures",
            "-R",
            &requirement,
        ])
        .arg(path);
    command(run, verify)?.passed()?;
    ensure!(
        &entitlements(run, path)? == expected,
        "the operating system read different signed entitlements"
    );
    let mut inspect = Command::new("/usr/bin/codesign");
    inspect.args(["--display", "--verbose=4"]).arg(path);
    let observed = command(run, inspect)?;
    observed.passed()?;
    let stderr = fs::read_to_string(observed.directory.join("stderr.log"))?;
    let flags = stderr
        .lines()
        .find_map(|line| line.strip_prefix("CodeDirectory "))
        .and_then(|line| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix("flags=0x"))
        })
        .and_then(|value| value.split('(').next())
        .context("actual CodeDirectory flags are absent")?;
    ensure!(
        u32::from_str_radix(flags, 16)? & HARDENED_RUNTIME_FLAG != 0,
        "macOS reports no hardened runtime: {stderr}"
    );
    let cmd = run.product(&[
        "signing",
        "inspect",
        path.to_str().context("non-UTF8 test path")?,
        "--entitlements",
        "--json",
    ]);
    let reported = command(run, cmd)?.json()?;
    ensure!(
        reported[0]["hardened_runtime"] == true
            && reported[0]["entitlements"] == serde_json::to_value(expected)?,
        "the CLI does not report the actual signed policy: {reported}"
    );
    Ok(())
}

pub fn refusals(
    run: &mut Run,
    credentials: &Credentials,
    path: &Path,
    identifier: &str,
) -> Result<()> {
    let before = sha256(path)?;
    let target = path.to_str().context("non-UTF8 test path")?;
    let mut adhoc = run.product(&[
        "signing",
        "sign",
        target,
        "--identifier",
        identifier,
        "--identity",
        "-",
        "--json",
    ]);
    credentials.configure(&mut adhoc);
    command(run, adhoc)?.refused()?;
    ensure!(
        sha256(path)? == before,
        "an ad-hoc identity request changed the signed executable"
    );
    let mut unavailable = run.product(&[
        "signing",
        "sign",
        target,
        "--identifier",
        identifier,
        "--identity",
        "stado-unavailable-qualification-identity",
        "--json",
    ]);
    credentials.configure(&mut unavailable);
    command(run, unavailable)?.refused()?;
    ensure!(
        sha256(path)? == before,
        "an unavailable explicit identity changed the signed executable"
    );
    let changed = format!("{identifier}.changed");
    let mut rename = run.product(&[
        "signing",
        "sign",
        target,
        "--identifier",
        &changed,
        "--json",
    ]);
    credentials.configure(&mut rename);
    command(run, rename)?.refused()?;
    ensure!(
        sha256(path)? == before,
        "a refused identity change replaced the signed executable"
    );
    Ok(())
}
