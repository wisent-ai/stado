//! What a Rust checkout's own Cargo manifest says about the programs it
//! ships: the package name and version the release reads, and every binary
//! target, each of which the release builds and stages. Cargo answers through
//! `cargo metadata`, so the reading follows the manifest exactly as a build
//! would; a checkout Cargo cannot read, or one without a binary, is refused.

use std::path::Path;

use serde_json::{json, Map, Value};

use super::{fill, Planned, CARGO_BUILD};
use crate::cli::CmdError;
use crate::release_pipeline::PRODUCT_MANIFEST;

/// The platforms a Rust product is released for, as the fleet's builders
/// declare them.
const PLATFORMS: [&str; 2] = ["darwin-arm64", "linux-amd64"];

/// The manifest and script a Rust checkout is released with: every binary
/// target is built and staged and the version is read from Cargo.toml. No
/// post-build test is declared until the operator approves one.
pub(super) fn files(checkout: &Path, product: &str) -> Result<Vec<Planned>, CmdError> {
    let package = read(checkout)?;
    if package.name != product {
        return Err(CmdError::refused(format!(
            "Cargo.toml names the package {}, but the product is {product}; pass --product {}",
            package.name, package.name
        )));
    }
    let mut stage = Map::new();
    for binary in &package.binaries {
        stage.insert(format!("bin/{binary}"), json!(format!("bin/{binary}")));
    }
    for evidence in ["evidence/build.jsonl", "evidence/DIGESTS"] {
        stage.insert(evidence.to_string(), json!(evidence));
    }
    let platform = json!({
        "quality": [],
        "build": {"argv": ["bash", "release/build.sh"]},
        "stage": stage,
    });
    let platforms: Map<String, Value> = PLATFORMS
        .iter()
        .map(|name| {
            let mut entry = platform.clone();
            entry["runner_platform"] = json!(name);
            (name.to_string(), entry)
        })
        .collect();
    let manifest = json!({
        "schema_version": 1,
        "product": product,
        "releases": true,
        "version_source": {
            "kind": "regex",
            "path": "Cargo.toml",
            "pattern": "(?m)^version\\s*=\\s*\"(?P<version>[0-9]+\\.[0-9]+\\.[0-9]+)\"\\s*$"
        },
        "platforms": platforms,
        "promotion": {"channels": ["candidate"], "reconcile": false}
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(CmdError::from)? + "\n";
    let binaries = package.binaries.join(" ");
    let values = [("PRODUCT", product), ("BINARIES", binaries.as_str())];
    eprintln!(
        "{product}: Cargo.toml version {}, binaries {}",
        package.version, binaries
    );
    Ok(vec![
        Planned {
            path: checkout.join(PRODUCT_MANIFEST),
            text,
            executable: false,
        },
        Planned {
            path: checkout.join("release/build.sh"),
            text: fill(CARGO_BUILD, &values),
            executable: true,
        },
    ])
}

pub(super) struct Package {
    pub name: String,
    pub version: String,
    pub binaries: Vec<String>,
}

pub(super) fn read(checkout: &Path) -> Result<Package, CmdError> {
    let manifest = checkout.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(CmdError::refused(format!(
            "{} has no Cargo.toml at its root; --kind cargo reads the package from it",
            checkout.display()
        )));
    }
    let output = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .map_err(|error| CmdError::click(format!("cargo metadata could not start: {error}")))?;
    if !output.status.success() {
        return Err(CmdError::refused(format!(
            "cargo metadata refused {}: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        CmdError::click(format!("cargo metadata answered unreadable JSON: {error}"))
    })?;
    let root = std::fs::canonicalize(&manifest)?;
    let package = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|package| {
            package["manifest_path"]
                .as_str()
                .and_then(|path| std::fs::canonicalize(path).ok())
                .is_some_and(|path| path == root)
        })
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} is a workspace without a root package; adopt the member that ships the program",
                manifest.display()
            ))
        })?;
    let text = |key: &str| package[key].as_str().unwrap_or_default().to_string();
    let binaries: Vec<String> = package["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| {
            target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        })
        .filter_map(|target| target["name"].as_str().map(str::to_string))
        .collect();
    let name = text("name");
    if binaries.is_empty() {
        return Err(CmdError::refused(format!(
            "{name} declares no binary target; a release ships programs"
        )));
    }
    Ok(Package {
        name,
        version: text("version"),
        binaries,
    })
}
