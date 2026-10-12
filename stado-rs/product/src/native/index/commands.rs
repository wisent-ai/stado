use crate::native::protocol::{language, SOURCE_FILE_KIND};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub fn uri(path: &Path) -> Result<String> {
    url::Url::from_file_path(path)
        .map(|url| url.into())
        .map_err(|_| anyhow::anyhow!("path has no local file URI: {}", path.display()))
}

/// The compiler targets of `manifest`: every command whose declared inputs
/// include a source file of a language the index serves. Which program the
/// command runs is not read: a list of compiler names (`clang`, `swiftc`,
/// `swift-frontend`) decided this before and missed every toolchain spelled
/// otherwise. A source under `scratch`, the build tree SwiftPM wrote, is
/// generated, unless it lies in a dependency clone SwiftPM keeps there, which
/// is refused; every other source must belong to one of the package graph's
/// canonical checkouts.
pub fn targets(
    package: &Path,
    manifest: &Path,
    sources: &Value,
    scratch: &Path,
) -> Result<Vec<Value>> {
    let document: Value = serde_yaml::from_slice(&fs::read(manifest)?)?;
    let commands = document["commands"].as_object().with_context(|| {
        format!(
            "native compiler manifest has no command map: {}",
            manifest.display()
        )
    })?;
    let roots: Vec<PathBuf> = sources
        .as_array()
        .context("native report has no package sources")?
        .iter()
        .map(|source| {
            source["path"]
                .as_str()
                .map(PathBuf::from)
                .context("native source has no package path")
        })
        .collect::<Result<_>>()?;
    let mut targets = Vec::new();
    let mut producers = BTreeMap::new();
    let mut inputs = BTreeMap::new();
    for (name, command) in commands {
        let Some((compiler, compiler_arguments)) = command["args"]
            .as_array()
            .and_then(|arguments| arguments.split_first())
        else {
            continue;
        };
        let compiler = compiler
            .as_str()
            .context("compiler argument must be a string")?;
        let declared_inputs = command["inputs"]
            .as_array()
            .context("compiler command has no input array")?;
        let files: Vec<_> = declared_inputs
            .iter()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .filter(|path| {
                path.extension()
                    .and_then(|s| s.to_str())
                    .and_then(language)
                    .is_some()
            })
            .collect();
        if files.is_empty() {
            continue;
        }
        let mut indexed = Vec::new();
        let mut languages = BTreeSet::new();
        for path in files {
            let generated = path.starts_with(scratch);
            if generated {
                if let Some(clone) = super::clone::repository_inside(&path, scratch)? {
                    bail!(
                        "compiler source belongs to a dependency clone {}: {}",
                        clone.display(),
                        path.display()
                    );
                }
            } else if !roots.iter().any(|root| path.starts_with(root)) {
                bail!(
                    "compiler source is outside the canonical package graph and the build tree \
                     {}: {}",
                    scratch.display(),
                    path.display()
                );
            }
            languages.insert(language(path.extension().and_then(|s| s.to_str()).unwrap()).unwrap());
            indexed.push(
                json!({"uri": uri(&path)?, "kind": SOURCE_FILE_KIND, "generated": generated}),
            );
        }
        let identifier = format!(
            "wisent-native:///{}",
            hex::encode(Sha256::digest(name.as_bytes()))
        );
        targets.push(json!({"id": {"uri": identifier}, "displayName": name, "languageIds": languages,
            "arguments": compiler_arguments, "compiler": compiler, "workingDirectory": package, "sources": indexed}));
        inputs.insert(identifier.clone(), declared_inputs.clone());
        if let Some(outputs) = command["outputs"].as_array() {
            for output in outputs {
                producers.insert(
                    output
                        .as_str()
                        .context("compiler output must be a string")?
                        .to_owned(),
                    identifier.clone(),
                );
            }
        }
    }
    if targets.is_empty() {
        bail!(
            "no supported Swift or C compiler commands in {}",
            manifest.display()
        );
    }
    for target in &mut targets {
        let identifier = target["id"]["uri"].as_str().unwrap();
        let mut dependencies = BTreeSet::new();
        for input in &inputs[identifier] {
            if let Some(producer) = input.as_str().and_then(|input| producers.get(input)) {
                if producer != identifier {
                    dependencies.insert(producer.clone());
                }
            }
        }
        target["dependencies"] = json!(dependencies
            .into_iter()
            .map(|uri| json!({"uri": uri}))
            .collect::<Vec<_>>());
    }
    Ok(targets)
}
