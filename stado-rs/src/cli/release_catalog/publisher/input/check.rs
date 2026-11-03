//! `stado release catalog input check`: whether a product's published
//! `private-cargo-sources` input still carries exactly the Git packages its
//! `Cargo.lock` locks. A commit that moves a private Git package in the lock
//! leaves the input behind, and every release build of that commit is then
//! refused with `private Cargo sources do not match Cargo.lock`; this reads
//! the same comparison before anything is built, so a quality gate or a push
//! check refuses the commit and names the pin that repairs it.

use std::path::PathBuf;

use serde_json::json;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

#[derive(clap::Args)]
pub(in crate::cli::release_catalog) struct CheckInputArgs {
    /// The product checkout whose `.wisent-release.json` and `Cargo.lock` are
    /// compared.
    checkout: PathBuf,
    #[arg(long)]
    json: bool,
}

pub(in crate::cli::release_catalog) async fn check_input(
    args: CheckInputArgs,
) -> Result<(), CmdError> {
    let CheckInputArgs { checkout, json } = args;
    let name = stado_product::PRIVATE_CARGO_INPUT_NAME;
    let (manifest_path, manifest, product) = super::manifest::read(&checkout)?;
    let lock = checkout.join("Cargo.lock");
    let declared = manifest["inputs"][name]["uri"].as_str().map(str::to_owned);
    let pin = format!(
        "stado release catalog input pin {} --name {name} --source {} --revision <commit> --cargo",
        checkout.display(),
        checkout.display()
    );
    let difference = match (&declared, lock.is_file()) {
        (None, false) => None,
        (None, true) => locks_private_git(&checkout)?.then(|| {
            format!(
                "{} declares no {name} input and Cargo.lock locks private Git packages",
                manifest_path.display()
            )
        }),
        (Some(uri), _) => {
            let archive = crate::cli::storage::fetch_object(uri)
                .await
                .map_err(|error| {
                    CmdError::click(format!(
                        "{product}'s {name} input {uri} cannot be read: {error}"
                    ))
                })?;
            stado_product::private_cargo_lock_difference(&archive, &checkout)
                .map_err(|error| {
                    CmdError::click(format!(
                        "{product}'s {name} input {uri} cannot be compared with {}: {error:#}",
                        lock.display()
                    ))
                    .stating(FailureCode::Config)
                })?
                .map(|said| format!("{product}'s {name} input {uri} is stale: {said}"))
        }
    };
    let report = json!({
        "product": product,
        "input": name,
        "uri": declared,
        "matches": difference.is_none(),
        "difference": difference,
        "repair": difference.as_ref().map(|_| pin.clone()),
    });
    match &difference {
        None => {
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("{product}: {name} matches {}", lock.display());
            }
            Ok(())
        }
        Some(said) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            Err(CmdError::click(format!(
                "{said}; every release build of this commit would be refused; publish the input \
                 the lock names with `{pin}` and commit .wisent-release.json"
            ))
            .stating(FailureCode::Config))
        }
    }
}

/// Whether a checkout's `Cargo.lock` locks any Git package: one that does
/// needs the input, one that does not needs none.
fn locks_private_git(checkout: &std::path::Path) -> Result<bool, CmdError> {
    let lock = checkout.join("Cargo.lock");
    let text = std::fs::read_to_string(&lock)
        .map_err(|error| CmdError::click(format!("{}: {error}", lock.display())))?;
    Ok(text.lines().any(|line| line.starts_with("source = \"git+")))
}
