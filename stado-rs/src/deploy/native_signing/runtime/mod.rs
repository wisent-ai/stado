//! One qualified native SDK release for both product commands and code signing.
//! The immutable release coordinate, not a Python environment or PATH entry,
//! selects the implementation. Cache reuse checks the actual executable bytes.

mod artifact;
mod cache;
mod remote;

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::deploy::{CommandSpec, DeployError, Runner};
use crate::release_control::ReleaseArtifactRef;
use crate::targets::ComputeTarget;

pub const VERSION: &str = "0.5.0";
const PRODUCT: &str = "wisent-products";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    version: String,
    platform: String,
    artifact: ReleaseArtifactRef,
    executable_bytes: u64,
    executable_sha256: String,
}

struct Prepared {
    program: PathBuf,
    receipt: Receipt,
}

fn platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("linux", "x86_64") => Ok("linux-amd64"),
        (os, arch) => bail!("native Wisent Products {VERSION} has no release for {os}/{arch}"),
    }
}

fn program(home: &str, platform: &str) -> String {
    format!("{home}/.stado/cache/product-sdk/{VERSION}/{platform}/{PRODUCT}")
}

async fn prepare(platform: &str) -> Result<Prepared> {
    let (root, _lock) = cache::root(platform)?;
    if let Some(prepared) = cache::read(&root, platform)? {
        return Ok(prepared);
    }
    artifact::install(&root, platform).await
}

fn expected_version(receipt: &Receipt) -> String {
    format!(
        "{PRODUCT} {VERSION}\nsource {}",
        receipt.artifact.source_revision
    )
}

fn checked_version(output: &crate::deploy::CommandOutput, receipt: &Receipt) -> Result<()> {
    ensure!(
        output.ok() && output.stdout.trim() == expected_version(receipt),
        "native SDK execution differs from its qualified release: expected {:?}; observed exit {}, stdout {:?}, stderr {:?}",
        expected_version(receipt), output.code, output.stdout, output.stderr
    );
    Ok(())
}

fn observed(prepared: &Prepared, program: &str) {
    eprintln!(
        "native Wisent Products {VERSION}: source {}, executable sha256 {}, program {program}",
        prepared.receipt.artifact.source_revision, prepared.receipt.executable_sha256
    );
}

pub async fn local(runner: &Runner) -> Result<PathBuf, DeployError> {
    async {
        let prepared = prepare(platform()?).await?;
        let program = prepared
            .program
            .to_str()
            .context("native SDK path is not UTF-8")?;
        let output = runner(CommandSpec::new(vec![program.into(), "--version".into()]))
            .await
            .map_err(anyhow::Error::msg)?;
        checked_version(&output, &prepared.receipt)?;
        observed(&prepared, program);
        Ok(prepared.program)
    }
    .await
    .map_err(|error: anyhow::Error| {
        DeployError(format!("native SDK preparation failed: {error:#}"))
    })
}

pub async fn on_host(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    async {
        let prepared = prepare("darwin-arm64").await?;
        remote::install(target, home, &prepared, runner).await
    }
    .await
    .map_err(|error: anyhow::Error| {
        DeployError(format!(
            "{}: native SDK preparation failed: {error:#}",
            target.name
        ))
    })
}
