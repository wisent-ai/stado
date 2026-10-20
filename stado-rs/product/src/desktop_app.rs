//! Restart a macOS application bundle through LaunchServices after its bytes
//! have been replaced: `stado product restart`, and the desktop surface's
//! installation once it has placed its bundle. This is a local desktop
//! lifecycle operation, not a fleet service restart or a build step.

use anyhow::{bail, Context, Result};
#[cfg(target_os = "macos")]
use serde_json::json;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "macos")]
use crate::common::checked;

#[cfg(target_os = "macos")]
fn answer(command: &mut Command, operation: &str) -> Result<String> {
    let output = checked(command).with_context(|| operation.to_owned())?;
    String::from_utf8(output.stdout)
        .with_context(|| format!("{operation} answered non-UTF-8 output"))
}

#[cfg(target_os = "macos")]
fn running(bundle_id: &str) -> Result<String> {
    answer(
        Command::new("/usr/bin/lsappinfo")
            .arg("find")
            .arg(format!("bundleid={bundle_id}")),
        &format!("finding running application {bundle_id} through LaunchServices"),
    )
}

#[cfg(target_os = "macos")]
fn restart(bundle: &Path, only_if_running: bool) -> Result<serde_json::Value> {
    let info = bundle.join("Contents/Info.plist");
    if !bundle.is_dir() {
        bail!("application bundle not found: {}", bundle.display());
    }
    if !info.is_file() {
        bail!("application Info.plist not found: {}", info.display());
    }
    let bundle_id = answer(
        Command::new("/usr/bin/plutil")
            .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
            .arg(&info),
        &format!("reading CFBundleIdentifier from {}", info.display()),
    )?;
    let bundle_id = bundle_id.trim();
    if bundle_id.is_empty() {
        bail!("CFBundleIdentifier is empty in {}", info.display());
    }
    let was_running = !running(bundle_id)?.trim().is_empty();
    if !was_running && only_if_running {
        return Ok(json!({"bundle": bundle, "bundle_id": bundle_id, "state": "not-running"}));
    }
    if was_running {
        let response = answer(
            Command::new("/usr/bin/lsappinfo").args(["info", "-only", "pid", bundle_id]),
            &format!("reading LaunchServices process id for {bundle_id}"),
        )?;
        let pid_text = response
            .lines()
            .find_map(|line| line.strip_prefix("\"pid\"="))
            .with_context(|| {
                format!(
                    "LaunchServices lists {bundle_id} as running but reports no process id: {response:?}"
                )
            })?;
        let pid = pid_text.parse::<u32>().with_context(|| {
            format!(
                "LaunchServices reports invalid process id {pid_text:?} for {bundle_id}: {response:?}"
            )
        })?;
        checked(Command::new("/usr/bin/lsappinfo").args(["kill", bundle_id]))
            .with_context(|| format!("asking LaunchServices to terminate {bundle_id}"))?;
        let waiting = stado_wait::begin(
            stado_wait::Kind::Process,
            format!("{bundle_id} (pid {pid}) to exit"),
            "LaunchServices",
        );
        match checked(
            Command::new("/usr/bin/caffeinate")
                .arg("-w")
                .arg(pid.to_string()),
        ) {
            Ok(_) => waiting.done(),
            Err(error) => {
                waiting.failed(format!("{error:#}"));
                return Err(error).with_context(|| format!("waiting for {bundle_id} to exit"));
            }
        }
    }
    checked(Command::new("/usr/bin/open").arg("-g").arg(bundle))
        .with_context(|| format!("opening application bundle {}", bundle.display()))?;
    let observed = running(bundle_id)?;
    if observed.trim().is_empty() {
        bail!(
            "open accepted {}, but LaunchServices lists no running {bundle_id}",
            bundle.display()
        );
    }
    Ok(json!({
        "bundle": bundle,
        "bundle_id": bundle_id,
        "state": if was_running { "restarted" } else { "started" }
    }))
}

pub fn run(bundle: &Path, only_if_running: bool, as_json: bool) -> Result<i32> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (bundle, only_if_running, as_json);
        bail!("restarting a desktop application requires macOS LaunchServices");
    }
    #[cfg(target_os = "macos")]
    {
        let report = restart(bundle, only_if_running)?;
        if as_json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "{} {} ({})",
                report["state"]
                    .as_str()
                    .context("restart report has no state")?,
                bundle.display(),
                report["bundle_id"]
                    .as_str()
                    .context("restart report has no bundle id")?
            );
        }
        Ok(libc::EXIT_SUCCESS)
    }
}

/// Restart, where it runs, every application bundle a desktop installation
/// placed; a placed application that is not running stays stopped. Answers
/// each bundle's report.
pub fn restart_placed(paths: &[std::path::PathBuf]) -> Result<serde_json::Value> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = paths;
        bail!("restarting a desktop application requires macOS LaunchServices");
    }
    #[cfg(target_os = "macos")]
    {
        let mut reports = Vec::new();
        for bundle in paths
            .iter()
            .filter(|path| path.extension().is_some_and(|extension| extension == "app"))
        {
            reports.push(restart(bundle, true)?);
        }
        Ok(serde_json::Value::Array(reports))
    }
}

pub fn from_arguments(arguments: &clap::ArgMatches) -> Result<i32> {
    let bundle = arguments
        .get_one::<String>("bundle")
        .context("application bundle is missing")?;
    run(
        Path::new(bundle.as_str()),
        arguments.get_flag("if-running"),
        arguments.get_flag("json"),
    )
}
