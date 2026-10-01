//! A Swift package's public surface, read by the toolchain itself.
//!
//! `swift api-digester` serialises a module's public API as JSON and diagnoses
//! what one dump breaks against another. The surface is therefore the
//! compiler's own reading of the built module, never a scan of source text:
//! a declaration the compiler exports is in the dump, one it does not is not,
//! and a renamed, removed or retyped declaration is named by the digester as
//! the breakage it is.

use crate::common::{capture, checked};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The library product a package manifest declares, read from
/// `swift package describe --type json` rather than from manifest text.
pub fn library_module(package: &Path) -> Result<String> {
    let output = checked(
        Command::new("swift")
            .args(["package", "describe", "--type", "json", "--package-path"])
            .arg(package),
    )?;
    let description: Value = serde_json::from_slice(&output.stdout)
        .context("swift package describe did not answer with JSON")?;
    let products = description["products"]
        .as_array()
        .context("swift package describe lists no products")?;
    let mut libraries = products
        .iter()
        .filter(|product| product["type"].get("library").is_some());
    let library = libraries.next().with_context(|| {
        format!(
            "{}: declares no library product; a surface is a library's exported API",
            package.display()
        )
    })?;
    if libraries.next().is_some() {
        bail!(
            "{}: declares several library products; name one with --module",
            package.display()
        );
    }
    let targets = library["targets"]
        .as_array()
        .context("library product lists no targets")?;
    match targets.as_slice() {
        [one] => Ok(one
            .as_str()
            .context("library target is not a name")?
            .to_owned()),
        [] => bail!("library product lists no targets"),
        _ => bail!("library product spans several targets; name the module with --module"),
    }
}

/// Build the package and return the directory its `.swiftmodule` lands in.
pub fn build(package: &Path, scratch: &Path) -> Result<PathBuf> {
    let output = capture(
        Command::new("swift")
            .args(["build", "--package-path"])
            .arg(package)
            .arg("--scratch-path")
            .arg(scratch)
            .arg("--disable-keychain")
            .env("GIT_ALLOW_PROTOCOL", ""),
    )?;
    if !output.status.success() {
        bail!(
            "swift build of {} failed ({}): {}",
            package.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let binary = checked(
        Command::new("swift")
            .args(["build", "--package-path"])
            .arg(package)
            .arg("--scratch-path")
            .arg(scratch)
            .arg("--show-bin-path"),
    )?;
    let binary = PathBuf::from(String::from_utf8(binary.stdout)?.trim());
    let modules = binary.join("Modules");
    if modules.is_dir() {
        return Ok(modules);
    }
    Ok(binary)
}

/// Dump `module`'s public API to `output` as the digester's JSON.
pub fn dump(module: &str, modules: &Path, output: &Path) -> Result<()> {
    let result = capture(
        Command::new("swift")
            .args(["api-digester", "-dump-sdk", "-module", module, "-I"])
            .arg(modules)
            .args(["-avoid-location", "-avoid-tool-args", "-o"])
            .arg(output),
    )?;
    if !result.status.success() {
        bail!(
            "swift api-digester could not dump module {module} from {}: {}",
            modules.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    let text = std::fs::read_to_string(output)
        .with_context(|| format!("reading the surface dump {}", output.display()))?;
    let document: Value = serde_json::from_str(&text).context("the surface dump is not JSON")?;
    let children = document["children"].as_array().map_or(0, Vec::len);
    if children == 0 {
        bail!(
            "the digester produced an empty surface for {module}. A library that exports nothing is a defect in the read, not a fact about the package; refusing to freeze emptiness"
        );
    }
    Ok(())
}

/// Diagnose `candidate` against `baseline`; the digester's own report, and
/// whether it exited naming a breakage. The digester exits non-zero exactly
/// when it emitted an error, so its status is the verdict.
pub fn diagnose(baseline: &Path, candidate: &Path) -> Result<(String, bool)> {
    let result = capture(
        Command::new("swift")
            .args(["api-digester", "-diagnose-sdk", "-input-paths"])
            .arg(baseline)
            .arg("-input-paths")
            .arg(candidate),
    )?;
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    Ok((report, !result.status.success()))
}
