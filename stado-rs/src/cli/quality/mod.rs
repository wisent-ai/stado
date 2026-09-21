//! `stado quality format` — apply the formatting the product's own quality
//! gate checks.
//!
//! Every product declares its gates in `.wisent-release.json`, and the first
//! of them reads the tree without changing it: `cargo fmt … -- --check`. When
//! that gate refuses, something has to do the writing, and until 2026-09-21
//! that something was a person typing `cargo fmt` — a command whose effect is
//! a tree-wide rewrite nobody reviewed, which on that day swept three other
//! sessions' unformatted files into one commit. Tama refuses it for that
//! reason.
//!
//! So the writing belongs here, to the product, driven by the same declaration
//! the gate reads: the checking argv with its `--check` removed. A product
//! that declares no formatting gate is told so rather than guessed at, and the
//! command never invents a formatter the manifest does not name.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::CmdError;
use crate::release_pipeline::{self, PlatformRecipe, ProductManifest, QualityGate};

/// The manifest every product carries at its checkout root.
const MANIFEST: &str = ".wisent-release.json";

/// The name that marks the gate which reads formatting.
const FORMAT_GATE: &str = "fmt";

/// The argument that makes a formatter report instead of write.
const CHECK_FLAG: &str = "--check";

pub async fn format(root: Option<&str>) -> Result<(), CmdError> {
    let root = match root {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().map_err(|error| {
            CmdError::click(format!("cannot read the working directory: {error}"))
        })?,
    };
    let manifest_path = root.join(MANIFEST);
    let bytes = std::fs::read(&manifest_path).map_err(|error| {
        CmdError::click(format!(
            "cannot read {}: {error}; `stado quality format` runs the formatting a product \
             declares, so it needs the product's own manifest",
            manifest_path.display()
        ))
    })?;
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::click)?
    else {
        return Err(CmdError::click(format!(
            "{} declares releases:false, so it declares no quality gate to apply",
            manifest_path.display()
        )));
    };
    let recipe = recipe_for_this_host(&manifest.platforms)?;
    let gates: Vec<&QualityGate> = recipe
        .quality
        .iter()
        .filter(|gate| gate.name == FORMAT_GATE || gate.argv.iter().any(|arg| arg == FORMAT_GATE))
        .collect();
    if gates.is_empty() {
        return Err(CmdError::click(format!(
            "{} declares no formatting gate for this platform; there is nothing to apply",
            manifest_path.display()
        )));
    }
    for gate in gates {
        let argv = writing_argv(&gate.argv);
        println!("stado quality format: {}", argv.join(" "));
        run(&argv, &root)?;
    }
    println!(
        "stado quality format: {} formatted in {}",
        manifest.product,
        root.display()
    );
    Ok(())
}

/// The recipe this host can actually run, or the refusal that says why not.
fn recipe_for_this_host(
    platforms: &std::collections::BTreeMap<String, PlatformRecipe>,
) -> Result<&PlatformRecipe, CmdError> {
    let here =
        crate::cli::fleet::enroll::release_platform(std::env::consts::OS, std::env::consts::ARCH)
            .unwrap_or_default();
    if let Some(recipe) = platforms.get(here) {
        return Ok(recipe);
    }
    // rustfmt reads the same source and writes the same bytes on every
    // platform. A product built only for Linux is still formatted here.
    platforms.values().next().ok_or_else(|| {
        CmdError::click("the manifest declares no platform, so it declares no gates".to_string())
    })
}

/// The checking argv turned into the writing one: `--check` removed, and the
/// `--` separator dropped when nothing follows it.
fn writing_argv(argv: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = argv
        .iter()
        .filter(|arg| arg.as_str() != CHECK_FLAG)
        .cloned()
        .collect();
    if kept.last().map(String::as_str) == Some("--") {
        kept.pop();
    }
    kept
}

fn run(argv: &[String], root: &Path) -> Result<(), CmdError> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| CmdError::click("a quality gate declares an empty command"))?;
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|error| CmdError::click(format!("cannot run {program}: {error}")))?;
    if status.success() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} exited {}",
        argv.join(" "),
        status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "on a signal".to_string())
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declaration is the source of the writing command: the gate that
    /// reads with `--check` becomes the same command without it, and the
    /// trailing separator goes with it so `cargo fmt … --all` is what runs.
    #[test]
    fn the_checking_command_becomes_the_writing_one() {
        let argv: Vec<String> = [
            "cargo",
            "fmt",
            "--manifest-path",
            "stado-rs/Cargo.toml",
            "--all",
            "--",
            "--check",
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
        assert_eq!(
            writing_argv(&argv),
            vec![
                "cargo".to_string(),
                "fmt".to_string(),
                "--manifest-path".to_string(),
                "stado-rs/Cargo.toml".to_string(),
                "--all".to_string(),
            ]
        );
    }

    /// A gate that already writes is left exactly as declared.
    #[test]
    fn a_writing_gate_is_left_alone() {
        let argv: Vec<String> = ["swift-format", "--in-place", "--recursive", "Sources"]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        assert_eq!(writing_argv(&argv), argv);
    }
}
