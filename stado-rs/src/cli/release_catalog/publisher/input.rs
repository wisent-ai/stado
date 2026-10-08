//! Publish a committed repository tree or its locked private Cargo packages
//! as an immutable release input. Cargo inputs carry directory-source
//! checksums and provenance, so release workers need no Git credentials.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::cli::CmdError;

mod bundle;

const MANIFEST: &str = ".wisent-release.json";
const CONTENT_TYPE: &str = "application/gzip";

#[derive(clap::Args)]
pub(in crate::cli::release_catalog) struct PinInputArgs {
    /// The product checkout whose release manifest gains the input.
    checkout: PathBuf,
    /// Input name, mount and environment key.
    #[arg(long)]
    name: String,
    /// The repository whose committed source is published.
    #[arg(long)]
    source: PathBuf,
    /// Commit, tag or branch to pin.
    #[arg(long)]
    revision: String,
    /// Keep these repository paths; repeat for several.
    #[arg(long = "path")]
    paths: Vec<String>,
    /// Export Cargo.lock's private Git crates instead of the repository tree.
    /// Requires --name private-cargo-sources and committed Cargo manifests.
    #[arg(long, conflicts_with = "paths")]
    cargo: bool,
    /// Export the pinned commit and its reachable history as a Git bundle.
    /// Use with stado web quality/build --git-input NAME=OWNER/REPOSITORY.git.
    #[arg(long)]
    git_bundle: bool,
    #[arg(long)]
    json: bool,
}

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
            "git -C {} {} failed: {}{}",
            repository.display(),
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
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

pub(in crate::cli::release_catalog) async fn pin_input(args: PinInputArgs) -> Result<(), CmdError> {
    let PinInputArgs {
        checkout,
        name,
        source,
        revision,
        paths,
        cargo,
        git_bundle,
        json,
    } = &args;
    if *git_bundle && (*cargo || !paths.is_empty()) {
        return Err(CmdError::usage(
            "--git-bundle cannot be combined with --cargo or --path: a Git bundle carries the pinned commit and its reachable history",
        ));
    }
    if *cargo && name != stado_product::PRIVATE_CARGO_INPUT_NAME {
        return Err(CmdError::usage(format!(
            "--cargo requires --name {}",
            stado_product::PRIVATE_CARGO_INPUT_NAME
        )));
    }
    if !valid_name(name) {
        return Err(CmdError::usage(format!(
            "--name {name:?} must be lowercase letters, digits and hyphens: it names the \
             input, its mount and WISENT_INPUT_<NAME>_DIR"
        )));
    }
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
    if manifest
        .get("inputs")
        .is_some_and(|inputs| !inputs.is_object())
    {
        return Err(CmdError::usage(format!(
            "{}: inputs is not an object",
            manifest_path.display()
        )));
    }
    let commit = String::from_utf8_lossy(&git(
        source,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )?)
    .trim()
    .to_string();
    let cargo_archive;
    let git_archive;
    let bundle_archive;
    let (stored_path, digest) = if *cargo {
        git(
            source,
            &[
                "diff",
                "--exit-code",
                &commit,
                "--",
                "*.toml",
                "**/Cargo.lock",
                "Cargo.lock",
            ],
        )?;
        cargo_archive = stado_product::export_private_cargo_sources(source).map_err(|error| {
            CmdError::click(format!("private Cargo source export failed: {error:#}"))
        })?;
        git(
            source,
            &[
                "diff",
                "--exit-code",
                &commit,
                "--",
                "*.toml",
                "**/Cargo.lock",
                "Cargo.lock",
            ],
        )?;
        (
            cargo_archive.archive.as_path(),
            cargo_archive.sha256.clone(),
        )
    } else if *git_bundle {
        bundle_archive = bundle::export(source, &commit, &checkout.join(".build/release-input"))?;
        let digest = stado_product::common::sha256(bundle_archive.path())
            .map_err(|error| CmdError::click(format!("cannot hash Git bundle input: {error:#}")))?;
        (bundle_archive.path(), digest)
    } else {
        for path in paths {
            git(source, &["cat-file", "-e", &format!("{commit}:{path}")]).map_err(|_| {
                CmdError::usage(format!(
                    "--path {path:?} does not exist at {commit} of {}",
                    source.display()
                ))
            })?;
        }
        let prefix = format!("--prefix={name}/");
        let mut arguments = vec![
            "archive",
            "--format=tar.gz",
            prefix.as_str(),
            commit.as_str(),
        ];
        if !paths.is_empty() {
            arguments.push("--");
            arguments.extend(paths.iter().map(String::as_str));
        }
        let archive = git(source, &arguments)?;
        let scratch = checkout.join(".build/release-input");
        std::fs::create_dir_all(&scratch)?;
        let mut staged = tempfile::NamedTempFile::new_in(scratch)?;
        staged.write_all(&archive)?;
        git_archive = staged;
        let digest = stado_product::common::sha256(git_archive.path())
            .map_err(|error| CmdError::click(format!("cannot hash release input: {error:#}")))?;
        (git_archive.path(), digest)
    };
    let bytes = std::fs::metadata(stored_path)?.len();

    let (filename, content_type, mount, extract) = if *git_bundle {
        (
            "source.bundle",
            "application/x-git-bundle",
            format!("{name}.bundle"),
            false,
        )
    } else {
        ("source.tar.gz", CONTENT_TYPE, name.to_string(), true)
    };
    let uri = format!("stado://sources/{product}/dependencies/{name}/sha256/{digest}/{filename}");

    crate::cli::storage::store_object(&uri, &stored_path.to_string_lossy(), content_type, true)
        .await?;

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
            json!({"uri": uri, "sha256": digest, "mount": mount, "extract": extract}),
        );
    stado_product::common::atomic_json(&manifest_path, &manifest).map_err(|error| {
        CmdError::click(format!(
            "cannot write {}: {error:#}",
            manifest_path.display()
        ))
    })?;

    let report = json!({
        "product": product, "input": name, "source_commit": commit, "paths": paths,
        "cargo": cargo, "git_bundle": git_bundle, "uri": uri, "sha256": digest,
        "bytes": bytes, "manifest": manifest_path,
    });
    if *json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "pinned {name} at {commit} as {uri} in {}",
            manifest_path.display()
        );
    }
    Ok(())
}
