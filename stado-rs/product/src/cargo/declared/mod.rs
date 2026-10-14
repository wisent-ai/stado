//! Which Cargo operations `stado product cargo` runs on a tree.
//!
//! Its own operations — the values its command declaration names — it runs
//! on any tree. Beyond them, a product's release manifest
//! (`.wisent-release.json` at the tree's root) declares quality gates, and a
//! gate whose argv is `cargo <subcommand> …` is an operation the product's
//! installation runs on that tree; `stado product cargo` runs it too, with
//! the same locked, canonical sources, so a gate's refusal (a clippy lint,
//! say) can be read on the exact committed revision without running every
//! other gate. The declared set is read from the tree being built — the
//! exported revision under `--revision`.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::{collections::BTreeSet, path::Path};

/// The release manifest every product carries at its tree's root.
const MANIFEST: &str = ".wisent-release.json";

/// The program a gate's argv names when it is a Cargo operation.
const CARGO: &str = "cargo";

/// The manifest's kind for a gate that reads source text without compiling
/// it (`cargo fmt`); every other kind compiles the tree.
const FORMAT_KIND: &str = "format";

#[derive(Deserialize)]
struct Manifest {
    platforms: Option<std::collections::BTreeMap<String, Platform>>,
}

#[derive(Deserialize)]
struct Platform {
    quality: Option<Vec<Gate>>,
}

#[derive(Deserialize)]
struct Gate {
    argv: Vec<String>,
    kind: String,
}

/// The Cargo operations of the manifest at `tree`'s gates, split into those
/// that compile the tree and those that only read its text.
#[derive(Default)]
pub(crate) struct GateOperations {
    pub(crate) compiling: BTreeSet<String>,
    pub(crate) formatting: BTreeSet<String>,
}

/// Every Cargo subcommand a quality gate of the manifest at `tree` runs. A
/// tree with no manifest declares none; a manifest that cannot be read or
/// does not parse is refused, naming the file.
pub(crate) fn gate_operations(tree: &Path) -> Result<GateOperations> {
    let path = tree.join(MANIFEST);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GateOperations::default())
        }
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };
    let manifest: Manifest = serde_json::from_str(&text)
        .with_context(|| format!("{} does not parse as a release manifest", path.display()))?;
    let mut operations = GateOperations::default();
    let gates = manifest
        .platforms
        .into_iter()
        .flatten()
        .flat_map(|(_, platform)| platform.quality.into_iter().flatten());
    for gate in gates {
        let [program, operation, ..] = gate.argv.as_slice() else {
            continue;
        };
        if program != CARGO {
            continue;
        }
        if gate.kind == FORMAT_KIND {
            operations.formatting.insert(operation.clone());
        } else {
            operations.compiling.insert(operation.clone());
        }
    }
    Ok(operations)
}

/// Admit `operation` on `tree`: one of `own` (the command's declared
/// operations), or a Cargo subcommand a compiling quality gate declared
/// there runs. A format gate's operation is refused with the command that
/// reads it — it compiles nothing and takes none of the canonical-source
/// flags this command adds — and any other with both admitted sets.
pub(crate) fn admit(tree: &Path, operation: &str, own: &[String]) -> Result<()> {
    if own.iter().any(|known| known == operation) {
        return Ok(());
    }
    let declared = gate_operations(tree)?;
    if declared.compiling.contains(operation) {
        return Ok(());
    }
    if declared.formatting.contains(operation) {
        bail!(
            "`cargo {operation}` is a format gate of {}: it compiles nothing, so `stado quality \
             check --revision <commit>` reads it on a committed revision and `stado quality \
             format` writes what it reads",
            tree.join(MANIFEST).display()
        );
    }
    let compiling: Vec<&str> = declared.compiling.iter().map(String::as_str).collect();
    bail!(
        "stado product cargo does not run `cargo {operation}` on {}: it runs its own operations \
         ({}) and the Cargo operations the tree's {MANIFEST} compiling quality gates declare ({})",
        tree.display(),
        own.join(", "),
        match compiling.as_slice() {
            [] => "none".to_string(),
            compiling => compiling.join(", "),
        }
    )
}
