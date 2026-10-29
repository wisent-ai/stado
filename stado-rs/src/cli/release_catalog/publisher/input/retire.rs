//! `stado release catalog input retire`: remove a `sources` input that no
//! product's release manifest names. Every product checkout under the root is
//! fetched and its `origin/main` manifest read, and so is every release
//! catalog entry a declared publisher has; the input is retired only when none
//! of them names it. A manifest that cannot be read refuses the retirement,
//! since the input might be named there.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::release_pipeline::PRODUCT_MANIFEST;

#[derive(clap::Args)]
pub(in crate::cli::release_catalog) struct RetireInputArgs {
    /// The input, as `stado://sources/<key>`.
    uri: String,
    /// The directory every product checkout is found under, as `stado release
    /// catalog sync --root` reads it.
    #[arg(long)]
    root: PathBuf,
    /// Read every manifest and name what it found, and retire nothing.
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
}

/// Whether `value` holds the string `uri` anywhere.
fn names(value: &Value, uri: &str) -> bool {
    match value {
        Value::String(text) => text == uri,
        Value::Array(items) => items.iter().any(|item| names(item, uri)),
        Value::Object(fields) => fields.values().any(|field| names(field, uri)),
        _ => false,
    }
}

/// `git -C checkout <arguments>` that never asks anyone for a credential.
fn git(checkout: &Path, arguments: &[&str]) -> Result<std::process::Output, CmdError> {
    crate::wait::output(
        Command::new("git")
            .arg("-C")
            .arg(checkout)
            .args(["-c", "credential.interactive=never", "-c", "core.askPass="])
            .args(arguments)
            .env("GIT_TERMINAL_PROMPT", "0"),
    )
    .map_err(|error| {
        CmdError::click(format!(
            "git -C {} {}: {error}",
            checkout.display(),
            arguments.join(" ")
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })
}

fn refused(checkout: &Path, step: &str, output: &std::process::Output) -> CmdError {
    CmdError::click(format!(
        "{} cannot be read at origin/main ({step} exited {}: {}), so whether it names the input \
         is not known; nothing was retired",
        checkout.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
    .stating(FailureCode::Config)
}

/// The release manifest `checkout`'s origin/main holds after a fetch, or
/// `None` when that commit has none.
fn origin_manifest(checkout: &Path) -> Result<Option<Value>, CmdError> {
    let fetched = git(checkout, &["fetch", "--quiet", "origin", "main"])?;
    if !fetched.status.success() {
        return Err(refused(checkout, "git fetch origin main", &fetched));
    }
    let head = git(
        checkout,
        &["rev-parse", "--verify", "--quiet", "origin/main^{commit}"],
    )?;
    if !head.status.success() {
        return Err(refused(checkout, "git rev-parse origin/main", &head));
    }
    let path = format!("origin/main:{PRODUCT_MANIFEST}");
    if !git(checkout, &["cat-file", "-e", &path])?.status.success() {
        return Ok(None);
    }
    let shown = git(checkout, &["show", &path])?;
    if !shown.status.success() {
        return Err(refused(checkout, "git show", &shown));
    }
    serde_json::from_slice(&shown.stdout)
        .map(Some)
        .map_err(|error| {
            CmdError::declaration(format!(
            "{} origin/main {PRODUCT_MANIFEST} is not JSON ({error}); whether it names the input \
             is not known, so nothing was retired",
            checkout.display()
        ))
        })
}

/// Where `uri` is named: each checkout's origin/main manifest and each
/// release catalog entry, as one line per reference.
async fn references(root: &Path, uri: &str) -> Result<(Vec<String>, usize), CmdError> {
    let mut found = Vec::new();
    let checkouts = crate::cli::release_newest::declaring_checkouts(&root.canonicalize()?)?;
    let mut read = 0;
    for checkout in &checkouts {
        if let Some(manifest) = origin_manifest(checkout)? {
            if names(&manifest, uri) {
                found.push(format!(
                    "{} origin/main {PRODUCT_MANIFEST}",
                    checkout.display()
                ));
            }
        }
        read += 1;
    }
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        CmdError::declaration(format!(
            "release_api.publishers cannot be read ({}), so the release catalog cannot be; \
             nothing was retired",
            problems.join("; ")
        ))
    })?;
    for product in publishers.keys() {
        let entry_uri = crate::cli::release_catalog::catalog_uri(product);
        let Some(bytes) = crate::cli::storage::fetch_object_versioned(&entry_uri).await? else {
            continue;
        };
        let entry: Value = serde_json::from_slice(&bytes.0).map_err(|error| {
            CmdError::declaration(format!(
                "release catalog entry {entry_uri} is not JSON ({error}); nothing was retired"
            ))
        })?;
        if names(&entry, uri) {
            found.push(format!("release catalog entry {entry_uri}"));
        }
        read += 1;
    }
    Ok((found, read))
}

pub(in crate::cli::release_catalog) async fn retire_input(
    args: RetireInputArgs,
) -> Result<(), CmdError> {
    let object = crate::remote::object_store::ObjectRef::parse(&args.uri)?;
    let uri = object.to_string();
    if crate::remote::object_store::ReleaseNamespace::parse(object.namespace())
        != Some(crate::remote::object_store::ReleaseNamespace::Sources)
        || object.key().contains(crate::queue::UPLOAD_PART_MARKER)
    {
        return Err(CmdError::refused(format!(
            "{uri} is not a sources input; only an object of stado://sources/ can be retired, and \
             the staged parts of an upload are discarded with `stado storage abort <target-uri>`"
        )));
    }
    let (found, read) = references(&args.root, &uri).await?;
    if !found.is_empty() {
        return Err(CmdError::refused(format!(
            "{uri} is named by {}; nothing was retired",
            found.join(", ")
        )));
    }
    if !args.dry_run {
        crate::cli::storage::retire_input_object(&uri).await?;
    }
    let state = if args.dry_run {
        "unreferenced"
    } else {
        "retired"
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"uri": uri, "state": state, "manifests_read": read})
            )?
        );
    } else {
        println!("{state} {uri}: no manifest of the {read} read names it");
    }
    Ok(())
}
