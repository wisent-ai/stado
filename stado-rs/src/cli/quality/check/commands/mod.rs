//! Audit the CLI artifact built from the exact committed source being checked.

mod attested;
mod audit;
mod help_grammar;
mod judged;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::{gates::DeclaredGates, Report};
use crate::cli::CmdError;
pub(super) use audit::inventory;
use audit::{audit, Audit};
use help_grammar::clap_children;
use judged::Reading;

fn cli_product(product: &str) -> Result<bool, CmdError> {
    let catalog = stado_product::catalog::embedded()
        .map_err(|cause| CmdError::declaration(format!("product catalog: {cause:#}")))?;
    let products = catalog["products"].as_array().ok_or_else(|| {
        CmdError::declaration("product catalog: products is not an array".to_owned())
    })?;
    // A product the catalog does not carry declares no CLI surface to audit.
    let Some(record) = products.iter().find(|record| record["id"] == product) else {
        return Ok(false);
    };
    let surfaces = record["surfaces"].as_array().ok_or_else(|| {
        CmdError::declaration(format!(
            "catalogued product {product} has no surfaces array"
        ))
    })?;
    Ok(surfaces.iter().any(|surface| surface["kind"] == "cli"))
}

fn help(binary: &Path, path: &[String]) -> Result<String, CmdError> {
    let output =
        crate::wait::output(Command::new(binary).args(path).arg("--help")).map_err(|cause| {
            CmdError::click(format!(
                "{} {:?} --help did not run: {cause}",
                binary.display(),
                path
            ))
        })?;
    if !output.status.success() {
        return Err(CmdError::refused(format!(
            "{} {:?} --help exited {}: {}",
            binary.display(),
            path,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(|cause| {
        CmdError::refused(format!(
            "{} {:?} --help is not UTF-8: {cause}",
            binary.display(),
            path
        ))
    })
}

/// Read the command tree under `path` of `binary` into `tree`, each command's
/// help by its name; a model reading the tree needed that obtained no
/// judgment ends the walk as [`Reading::Unanswered`] with its cause.
fn discover(
    binary: &Path,
    path: &mut Vec<String>,
    parent_help: Option<&str>,
    tree: &mut BTreeMap<String, String>,
) -> Result<Reading<()>, CmdError> {
    let text = help(binary, path)?;
    if parent_help.is_some_and(|parent| parent == text.as_str()) {
        return Err(CmdError::refused(format!(
            "{} {:?} --help repeated its parent; cannot establish the real command tree",
            binary.display(),
            path
        )));
    }
    let root = binary
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CmdError::refused(format!(
                "CLI artifact has no UTF-8 name: {}",
                binary.display()
            ))
        })?;
    let mut name = root.to_owned();
    for component in path.iter() {
        name.push(' ');
        name.push_str(component);
    }
    let children = if let Some(advertised) = clap_children(&text, &name) {
        advertised.into_iter().map(str::to_owned).collect()
    } else {
        match judged::children(&name, &text)? {
            Reading::Answered(children) => children,
            Reading::Unanswered(cause) => return Ok(Reading::Unanswered(cause)),
        }
    };
    let mut unique = BTreeSet::new();
    for child in &children {
        if child.is_empty()
            || child.contains(char::is_whitespace)
            || !text.contains(child)
            || !unique.insert(child.as_str())
        {
            return Err(CmdError::refused(format!(
                "{name}: invalid or repeated child {child:?} from CLI help"
            )));
        }
    }
    drop(unique);
    for child in children {
        path.push(child);
        let result = discover(binary, path, Some(&text), tree);
        path.pop();
        if let Reading::Unanswered(cause) = result? {
            return Ok(Reading::Unanswered(cause));
        }
    }
    tree.insert(name, text);
    Ok(Reading::Answered(()))
}

fn staged_binaries(
    output: &Path,
    stage: &BTreeMap<String, String>,
) -> Result<Vec<PathBuf>, CmdError> {
    let mut binaries = Vec::new();
    for source in stage.keys() {
        let path = output.join(source);
        let metadata = std::fs::symlink_metadata(&path).map_err(|cause| {
            CmdError::click(format!(
                "CLI stage {} cannot be read: {cause}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(CmdError::refused(format!(
                "CLI stage {} is a symlink, not the candidate's built executable",
                path.display()
            )));
        }
        use nix::sys::stat::Mode;
        // mode_t is u16 on macOS and u32 on Linux: the conversion is real on
        // one and the identity on the other.
        #[allow(clippy::useless_conversion)]
        let execute = u32::from((Mode::S_IXUSR | Mode::S_IXGRP | Mode::S_IXOTH).bits());
        if metadata.is_file() && NonZeroU32::new(metadata.permissions().mode() & execute).is_some()
        {
            binaries.push(path);
        }
    }
    if binaries.is_empty() {
        return Err(CmdError::refused(format!(
            "the CLI product stages no executable file under {}; declare its executable in the release stage map",
            output.display()
        )));
    }
    Ok(binaries)
}

/// The CLI command tree of `revision` of `checkout`, built from the exported
/// `tree` into `area`, judged by a model against the CLI rules; when no model
/// judgment is obtained, the tree stands on the commit's attestation
/// ([`attested::stands`]) and the report says it was not judged.
pub(super) fn check(
    declared: &DeclaredGates,
    tree: &Path,
    area: &Path,
    contract: &[(&str, String)],
    checkout: &Path,
    revision: &str,
    report: Report,
) -> Result<(), CmdError> {
    let failures = match audit(declared, tree, area, contract, report, true)? {
        Audit::NotCli => return Ok(()),
        Audit::Judged(failures) => failures,
        Audit::Unjudged(cause) => {
            attested::stands(checkout, revision, &cause)?;
            report.say(&format!(
                "stado quality check: {} CLI command tree was not judged ({cause}); it stands on \
                 the attestation {revision} carries, read by `tama attestation check`",
                declared.product
            ));
            return Ok(());
        }
    };
    if !failures.is_empty() {
        return Err(CmdError::refused(format!(
            "{} CLI command tree refuses:\n{}",
            declared.product,
            failures.join("\n")
        )));
    }
    report.say(&format!(
        "stado quality check: {} CLI command tree passed",
        declared.product
    ));
    Ok(())
}
