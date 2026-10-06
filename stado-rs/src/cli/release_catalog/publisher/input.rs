//! `stado release catalog pin-input CHECKOUT --name N --source DIR --revision REV [--path P]...`:
//! one committed revision of another repository, or only the paths of it a
//! build reads, becomes an immutable build input of the product in CHECKOUT.
//!
//! A product that builds against a sibling repository at a pinned commit
//! (lem-desktop against lem and oko) has no way to hand that tree to a Stado
//! build worker, whose source is only the product's own archive. This command
//! archives exactly that commit (`git archive --prefix=<name>/`, limited to
//! `--path` when given), stores it create-only at
//! `stado://sources/<product>/dependencies/<name>/sha256/<digest>/source.tar.gz`,
//! and writes the `inputs.<name>` entry of the product's `.wisent-release.json`
//! (extracted, mounted as `<name>`), so the build reads it from
//! `WISENT_INPUT_<NAME>_DIR`. Moving the pin is running it again with the new
//! revision; the object store refuses to replace an existing digest.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cli::CmdError;

const MANIFEST: &str = ".wisent-release.json";
const CONTENT_TYPE: &str = "application/gzip";

fn git(repository: &Path, arguments: &[&str]) -> Result<Vec<u8>, CmdError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .map_err(|error| {
            CmdError::click(format!("git {}: {error}", arguments.join(" ")))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "git -C {} {} failed: {}",
            repository.display(),
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok(output.stdout)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(in crate::cli::release_catalog) async fn pin_input(
    checkout: &Path,
    name: &str,
    source: &Path,
    revision: &str,
    paths: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    if !valid_name(name) {
        return Err(CmdError::usage(format!(
            "--name {name:?} must be lowercase letters, digits and hyphens: it names the \
             input, its mount and WISENT_INPUT_<NAME>_DIR"
        )));
    }
    let commit = String::from_utf8_lossy(&git(
        source,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )?)
    .trim()
    .to_string();
    // `--path` keeps only what the build reads: a crate in a repository that
    // also carries gigabytes of media (echo-web's whole tree archived to
    // 2.28 GB for a 51 KB crate) is an input no object read can return in
    // one answer. Each path must exist at the commit, so a typo is refused
    // rather than archived as nothing; the paths keep their place under the
    // mount.
    for path in paths {
        git(source, &["cat-file", "-e", &format!("{commit}:{path}")]).map_err(|_| {
            CmdError::usage(format!(
                "--path {path:?} does not exist at {commit} of {}",
                source.display()
            ))
        })?;
    }
    let prefix = format!("--prefix={name}/");
    let mut arguments = vec!["archive", "--format=tar.gz", prefix.as_str(), commit.as_str()];
    if !paths.is_empty() {
        arguments.push("--");
        arguments.extend(paths.iter().map(String::as_str));
    }
    let archive = git(source, &arguments)?;
    let digest = hex::encode(Sha256::digest(&archive));

    let manifest_path = checkout.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest_path).map_err(|error| {
        CmdError::click(format!("{}: {error}", manifest_path.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let mut manifest: Value = serde_json::from_str(&text).map_err(|error| {
        CmdError::click(format!("{}: {error}", manifest_path.display()))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let product = manifest["product"]
        .as_str()
        .ok_or_else(|| {
            CmdError::click(format!("{} names no product", manifest_path.display()))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .to_string();
    let uri =
        format!("stado://sources/{product}/dependencies/{name}/sha256/{digest}/source.tar.gz");

    let mut staged = tempfile::NamedTempFile::new()?;
    staged.write_all(&archive)?;
    let stored_path = staged.path().to_string_lossy().to_string();
    crate::cli::storage::store_object(&uri, &stored_path, CONTENT_TYPE, true).await?;

    let inputs = manifest
        .as_object_mut()
        .ok_or_else(|| {
            CmdError::click(format!("{} is not a JSON object", manifest_path.display()))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .entry("inputs")
        .or_insert_with(|| json!({}));
    inputs
        .as_object_mut()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: inputs is not an object",
                manifest_path.display()
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .insert(
            name.to_string(),
            json!({"uri": uri, "sha256": digest, "mount": name, "extract": true}),
        );
    std::fs::write(
        &manifest_path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )
    .map_err(|error| {
        CmdError::click(format!("{}: {error}", manifest_path.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;

    let report = json!({
        "product": product, "input": name, "source_commit": commit, "paths": paths,
        "uri": uri, "sha256": digest, "bytes": archive.len(), "manifest": manifest_path,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "pinned {name} at {commit} as {uri} in {}",
            manifest_path.display()
        );
    }
    Ok(())
}
