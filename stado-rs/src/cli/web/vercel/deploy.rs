//! `stado web vercel deploy`: the verified release's prebuilt output, deployed
//! to Vercel production, with a receipt naming what went where.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::json;

use super::{required, run, OUTPUT_ARCHIVE, RECORD_SCHEMA, VERCEL_CLI};
use crate::cli::CmdError;

pub(crate) fn deploy() -> Result<(), CmdError> {
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let release_sha256 = required("WISENT_RELEASE_SHA256")?;
    let token = required("VERCEL_TOKEN")?;
    let output = PathBuf::from(required("WISENT_OUTPUT_DIR")?);

    let (_, actual) = crate::release_control::sha256_file(&archive).map_err(CmdError::click)?;
    if actual != release_sha256 {
        return Err(CmdError::click(format!(
            "the release archive {} is {actual}, not the published {release_sha256}; nothing was deployed",
            archive.display()
        )));
    }
    std::fs::create_dir_all(&output)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", output.display())))?;
    let work = output.join(format!(".vercel-deploy-{}", uuid::Uuid::new_v4()));
    let result = deploy_in(&archive, &work, &token);
    let _ = std::fs::remove_dir_all(&work);
    let url = result?;

    let receipt = json!({
        "schema_version": RECORD_SCHEMA,
        "channel": "vercel-production",
        "product": required("WISENT_PRODUCT")?,
        "version": required("WISENT_VERSION")?,
        "platform": required("WISENT_PLATFORM")?,
        "release_uri": required("WISENT_RELEASE_URI")?,
        "release_sha256": release_sha256,
        "release_manifest_uri": required("WISENT_RELEASE_MANIFEST_URI")?,
        "release_manifest_sha256": required("WISENT_RELEASE_MANIFEST_SHA256")?,
        "deployment_url": url,
    });
    let written = output.join("vercel-production-receipt.json");
    std::fs::write(&written, format!("{receipt}\n"))
        .map_err(|error| CmdError::click(format!("cannot write {}: {error}", written.display())))?;
    println!("stado web vercel: deployed {url}");
    Ok(())
}

/// Unpack the inner output from the release archive into `work`, refusing
/// links and paths that leave it, and deploy it; answers the deployment URL.
fn deploy_in(archive: &Path, work: &Path, token: &str) -> Result<String, CmdError> {
    std::fs::create_dir_all(work)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", work.display())))?;
    let listing = capture(Command::new("tar").arg("-tzf").arg(archive))?;
    let entry = listing
        .lines()
        .find(|name| name.rsplit('/').next() == Some(OUTPUT_ARCHIVE))
        .ok_or_else(|| {
            CmdError::click(format!(
                "the release archive holds no {OUTPUT_ARCHIVE}; nothing was deployed"
            ))
        })?
        .to_string();
    run(Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(work)
        .arg(&entry))?;
    let inner = work.join(&entry);

    // A long listing starts with the entry type: l is a symbolic link, h a hard link.
    for line in capture(Command::new("tar").arg("-tvzf").arg(&inner))?.lines() {
        if line.starts_with('l') || line.starts_with('h') {
            return Err(CmdError::click(format!(
                "the Vercel output holds a link: {line}"
            )));
        }
    }
    for name in capture(Command::new("tar").arg("-tzf").arg(&inner))?.lines() {
        if name.starts_with('/') || name == ".." || name.starts_with("../") || name.contains("/../")
        {
            return Err(CmdError::click(format!(
                "the Vercel output holds an unsafe path: {name}"
            )));
        }
    }
    let root = work.join("root");
    std::fs::create_dir_all(&root)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", root.display())))?;
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&inner)
        .arg("-C")
        .arg(&root))?;

    let deployed = capture(
        Command::new("npx")
            .args([
                "--yes",
                VERCEL_CLI,
                "deploy",
                "--prebuilt",
                "--prod",
                "--yes",
            ])
            // The token travels in the environment the CLI reads, never in
            // argv, where every process on the host could read it.
            .env("VERCEL_TOKEN", token)
            .current_dir(&root),
    )?;
    deployed
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CmdError::click("vercel deploy printed no deployment URL"))
}

/// Run one program and answer its stdout, refusing a failure with its stderr.
fn capture(command: &mut Command) -> Result<String, CmdError> {
    let rendered = format!("{command:?}");
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| CmdError::click(format!("cannot run {rendered}: {error}")))?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{rendered} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
