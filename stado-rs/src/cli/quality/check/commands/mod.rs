//! Audit the CLI artifact built from the exact committed source being checked.

mod help_grammar;
mod judged;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::{gates::DeclaredGates, run, Report};
use crate::cli::CmdError;
use help_grammar::clap_children;

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

fn discover(
    binary: &Path,
    path: &mut Vec<String>,
    parent_help: Option<&str>,
    tree: &mut BTreeMap<String, String>,
) -> Result<(), CmdError> {
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
        judged::children(&name, &text)?
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
        result?;
    }
    tree.insert(name, text);
    Ok(())
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

pub(super) fn check(
    declared: &DeclaredGates,
    tree: &Path,
    area: &Path,
    contract: &[(&str, String)],
    report: Report,
) -> Result<(), CmdError> {
    if super::super::web::is_web_platform(&declared.platform) || !cli_product(&declared.product)? {
        return Ok(());
    }
    let output = area.join("cli-surface");
    std::fs::create_dir_all(&output).map_err(|cause| {
        CmdError::click(format!(
            "cannot create CLI artifact area {}: {cause}",
            output.display()
        ))
    })?;
    let mut env = contract.to_vec();
    env.push(("WISENT_OUTPUT_DIR", output.to_string_lossy().into_owned()));
    for (name, value) in &declared.env {
        env.push((name, value.clone()));
    }
    report.say(&format!(
        "stado quality check: building candidate CLI: {}",
        declared.build.join(" ")
    ));
    run(&declared.build, tree, &env, report).map_err(|cause| {
        cause.within(format!("building CLI command tree of {}", declared.product))
    })?;
    let mut command_tree = BTreeMap::new();
    for binary in staged_binaries(&output, &declared.stage)? {
        let mut path = Vec::new();
        discover(&binary, &mut path, None, &mut command_tree)?;
        super::super::units::qualify(&binary, declared, report)?;
    }
    let mut failures = Vec::new();
    for issue in judged::violations(&declared.product, &command_tree)? {
        if !command_tree.contains_key(&issue.command)
            || issue.primitive.trim().is_empty()
            || issue.reason.trim().is_empty()
        {
            return Err(CmdError::refused(format!(
                "{}: CLI audit named an unknown command or omitted its primitive/reason: {} -> {}: {}",
                declared.product, issue.command, issue.primitive, issue.reason
            )));
        }
        failures.push(format!(
            "command `{}` duplicates or fragments primitive `{}`: {}",
            issue.command, issue.primitive, issue.reason
        ));
    }
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
