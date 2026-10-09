//! The Node.js every Node product build Stado runs uses: the release
//! `stado-rs/data/work/node-runtime.json` declares, from Node's own
//! distribution.
//!
//! A darwin builder carries Node through Homebrew; a Linux builder carries
//! none, so a Node product's linux-amd64 release failed its first gate with
//! `npx: not found`. A host installs the declared release under
//! `~/.local/share/stado-node/<version>`, verified against the release's
//! `SHASUMS256.txt`, and links its programs into `~/.local/bin`, the directory
//! every step Stado runs looks a program up in first. A host whose install
//! fails says which step failed instead of building without Node.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use stado_wait as wait;

use crate::common::{capture, emit, sha256};

/// The declaration's path in the Stado repository, for messages.
pub const DECLARATION_PATH: &str = "stado-rs/data/work/node-runtime.json";
const DECLARATION: &str = include_str!("../../../data/work/node-runtime.json");
const SCHEMA: &str = "stado.node-runtime.v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub schema: String,
    /// The exact release every host runs, as Node names it.
    pub version: String,
    /// The distribution the release is fetched from.
    pub distribution: String,
    /// Why this release.
    pub source: String,
}

pub fn declaration() -> Result<Declaration> {
    let declaration: Declaration = serde_json::from_str(DECLARATION)
        .with_context(|| format!("{DECLARATION_PATH} is not a node-runtime declaration"))?;
    if declaration.schema != SCHEMA {
        bail!(
            "{DECLARATION_PATH} declares schema {}; this Stado reads {SCHEMA}",
            declaration.schema
        );
    }
    Ok(declaration)
}

/// This machine as Node's distribution names its builds.
fn platform() -> Result<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        other => bail!("Node.js publishes no build Stado installs for {other}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => bail!("Node.js publishes no build Stado installs for {other}"),
    };
    Ok(format!("{os}-{arch}"))
}

fn root(home: &Path) -> PathBuf {
    home.join(".local/share/stado-node")
}

/// The declared release's `node` when it is installed and answers with it.
fn installed(home: &Path, declaration: &Declaration) -> Option<PathBuf> {
    let node = root(home).join(&declaration.version).join("bin/node");
    let output = wait::output(Command::new(&node).arg("--version")).ok()?;
    (output.status.success()
        && String::from_utf8_lossy(&output.stdout).trim() == declaration.version)
        .then_some(node)
}

fn fetch(url: &str, destination: &Path) -> Result<()> {
    let output = capture(
        Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--output",
            ])
            .arg(destination)
            .arg(url),
    )
    .with_context(|| format!("fetching {url}"))?;
    if !output.status.success() {
        bail!(
            "fetching {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Download, verify and unpack the declared release under the root.
fn install(home: &Path, declaration: &Declaration) -> Result<()> {
    let platform = platform()?;
    let version = &declaration.version;
    let archive_name = format!("node-{version}-{platform}.tar.gz");
    let base = format!("{}/{version}", declaration.distribution);
    let root = root(home);
    let staging = root.join(format!(".install-{}", std::process::id()));
    std::fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;
    let result = (|| -> Result<()> {
        let archive = staging.join(&archive_name);
        let sums = staging.join("SHASUMS256.txt");
        fetch(&format!("{base}/{archive_name}"), &archive)?;
        fetch(&format!("{base}/SHASUMS256.txt"), &sums)?;
        let listed = std::fs::read_to_string(&sums)?;
        let expected = listed
            .lines()
            .find_map(|line| line.strip_suffix(&format!("  {archive_name}")))
            .with_context(|| format!("{base}/SHASUMS256.txt lists no {archive_name}"))?;
        let actual = sha256(&archive)?;
        if actual != expected {
            bail!("{archive_name} has sha256 {actual}; {base}/SHASUMS256.txt lists {expected}");
        }
        let unpacked = capture(
            Command::new("tar")
                .arg("-xzf")
                .arg(&archive)
                .arg("-C")
                .arg(&staging),
        )?;
        if !unpacked.status.success() {
            bail!(
                "unpacking {archive_name}: {}",
                String::from_utf8_lossy(&unpacked.stderr).trim()
            );
        }
        let tree = staging.join(format!("node-{version}-{platform}"));
        let destination = root.join(version);
        if destination.exists() {
            std::fs::remove_dir_all(&destination)
                .with_context(|| format!("removing {}", destination.display()))?;
        }
        std::fs::rename(&tree, &destination)
            .with_context(|| format!("placing {}", destination.display()))
    })();
    std::fs::remove_dir_all(&staging).with_context(|| format!("removing {}", staging.display()))?;
    result
}

/// Link every program the release ships into `~/.local/bin`.
fn link(home: &Path, declaration: &Declaration) -> Result<Vec<PathBuf>> {
    let bin = root(home).join(&declaration.version).join("bin");
    let links = home.join(".local/bin");
    std::fs::create_dir_all(&links).with_context(|| format!("creating {}", links.display()))?;
    let mut linked = Vec::new();
    for entry in std::fs::read_dir(&bin).with_context(|| format!("reading {}", bin.display()))? {
        let program = entry?.path();
        let name = program.file_name().context("a program has a name")?;
        let link = links.join(name);
        if link.symlink_metadata().is_ok() {
            std::fs::remove_file(&link).with_context(|| format!("replacing {}", link.display()))?;
        }
        std::os::unix::fs::symlink(&program, &link)
            .with_context(|| format!("linking {}", link.display()))?;
        linked.push(link);
    }
    Ok(linked)
}

/// Install the declared release when it is absent or another, and link it.
pub fn ensure(home: &Path) -> Result<Value> {
    let declaration = declaration()?;
    if installed(home, &declaration).is_none() {
        install(home, &declaration)?;
    }
    let node = installed(home, &declaration).with_context(|| {
        format!(
            "{} was installed but does not answer {}",
            root(home)
                .join(&declaration.version)
                .join("bin/node")
                .display(),
            declaration.version
        )
    })?;
    let linked = link(home, &declaration)?;
    Ok(
        json!({"state": "ready", "node": node, "version": declaration.version,
        "linked": linked, "declared_by": DECLARATION_PATH}),
    )
}

pub fn status(home: &Path) -> Result<Value> {
    let declaration = declaration()?;
    let node = root(home).join(&declaration.version).join("bin/node");
    let state = if installed(home, &declaration).is_some() {
        "ready"
    } else {
        "absent"
    };
    Ok(
        json!({"state": state, "node": node, "version": declaration.version,
        "declared_by": DECLARATION_PATH}),
    )
}

/// `stado product node-runtime status|ensure [--json]`.
pub fn run(operation: &str, json_output: bool, home: &Path) -> Result<i32> {
    let report = match operation {
        "status" => status(home)?,
        "ensure" => ensure(home)?,
        other => bail!("unknown node-runtime operation {other}; use status or ensure"),
    };
    if json_output {
        emit(&report)?;
    } else {
        println!(
            "node {} {} at {}",
            report["version"], report["state"], report["node"]
        );
    }
    Ok(if report["state"] == "ready" { 0 } else { 1 }) // https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/stdlib.h.html
}
