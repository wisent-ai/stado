//! `stado product npm pack` and `stado product deliver npm`.
//!
//! An npm package is packed by npm itself, as a Rust one is built by Cargo;
//! what a product's copied release scripts added around it — exactly one
//! packed artifact, its digest, the verified release it is published from,
//! a publish that runs no package scripts, and the receipt — lives here once.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::python::{find, safe_unpack};
use super::{output_dir, required, RECORD_SCHEMA};

/// The packed package a build stages and a delivery looks for in the release.
pub const PACKAGE: &str = "npm-package.tgz";

/// `stado product npm pack`: `npm pack --ignore-scripts` of the checkout,
/// staged as `release/npm-package.tgz` with its SHA-256 beside it.
pub fn pack() -> Result<i32> {
    let source = PathBuf::from(required("WISENT_SOURCE_DIR")?);
    let release = output_dir()?.join("release");
    if release.exists() {
        fs::remove_dir_all(&release)?;
    }
    fs::create_dir_all(&release)?;
    let status = Command::new("npm")
        .args(["pack", "--ignore-scripts", "--pack-destination"])
        .arg(&release)
        .current_dir(&source)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .context("cannot run npm pack")?;
    if !status.success() {
        bail!("npm pack in {} failed with {status}", source.display());
    }
    let packed: Vec<PathBuf> = fs::read_dir(&release)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "tgz"))
        .collect();
    let [artifact] = packed.as_slice() else {
        bail!("npm pack produced {} artifacts; exactly one is required", packed.len());
    };
    let package = release.join(PACKAGE);
    fs::rename(artifact, &package)?;
    let digest = crate::common::sha256(&package)?;
    fs::write(release.join(format!("{PACKAGE}.sha256")), format!("{digest}\n"))?;
    println!("staged {} ({digest})", package.display());
    Ok(0)
}

/// `stado product deliver npm`: publish the package inside the verified
/// release archive unchanged, with `NPM_TOKEN`, and write `npm-receipt.json`.
pub fn deliver() -> Result<i32> {
    let token = required("NPM_TOKEN")?;
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let release_sha256 = required("WISENT_RELEASE_SHA256")?;
    let actual = crate::common::sha256(&archive)?;
    if actual != release_sha256 {
        bail!(
            "the release archive {} is {actual}, not the published {release_sha256}; nothing was published",
            archive.display()
        );
    }
    let output = output_dir()?;
    let work = output.join(format!("npm-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work)?;
    let result = (|| -> Result<Value> {
        safe_unpack(&archive, &work)?;
        let package = match find(&work, PACKAGE)?.as_slice() {
            [package] => package.clone(),
            found => bail!("the release holds {} {PACKAGE} (one is required)", found.len()),
        };
        let userconfig = work.join("npmrc");
        fs::write(&userconfig, "//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n")?;
        let published = Command::new("npm")
            .args(["publish", "--access", "public", "--ignore-scripts", "--json"])
            .arg(&package)
            .env("NPM_CONFIG_USERCONFIG", &userconfig)
            .env("NPM_TOKEN", &token)
            .stdin(Stdio::null())
            .output()
            .context("cannot run npm publish")?;
        if !published.status.success() {
            bail!(
                "npm publish failed with {}: {}",
                published.status,
                String::from_utf8_lossy(&published.stderr).trim()
            );
        }
        serde_json::from_slice(&published.stdout).context("npm publish answered non-JSON")
    })();
    let _ = fs::remove_dir_all(&work);
    let provider = result?;
    let receipt = json!({
        "schema_version": RECORD_SCHEMA, "channel": "npm",
        "product": required("WISENT_PRODUCT")?, "version": required("WISENT_VERSION")?,
        "release_uri": required("WISENT_RELEASE_URI")?, "release_sha256": release_sha256,
        "provider": provider,
    });
    fs::write(output.join("npm-receipt.json"), format!("{receipt}\n"))?;
    println!("published {PACKAGE} to npm");
    Ok(0)
}
