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

pub fn targets(package: &Path, manifest: &Path, sources: &Value) -> Result<Vec<Value>> {
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
        let Some(arguments) = command["args"]
            .as_array()
            .filter(|arguments| !arguments.is_empty())
        else {
            continue;
        };
        let compiler = arguments[0]
            .as_str()
            .context("compiler argument must be a string")?;
        let executable = Path::new(compiler)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if command["tool"] != "clang" && executable != "swiftc" && executable != "swift-frontend" {
            continue;
        }
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
            let root = roots
                .iter()
                .filter(|root| path.starts_with(root))
                .max_by_key(|root| root.components().count())
                .with_context(|| {
                    format!(
                        "compiler source is outside the canonical package graph: {}",
                        path.display()
                    )
                })?;
            let relative = path.strip_prefix(root)?;
            let components: Vec<_> = relative.components().map(|part| part.as_os_str()).collect();
            if components.first().is_some_and(|part| *part == ".build")
                && components
                    .iter()
                    .skip(1)
                    .any(|part| *part == "checkouts" || *part == "repositories")
            {
                bail!(
                    "compiler source belongs to a dependency clone: {}",
                    path.display()
                );
            }
            languages.insert(language(path.extension().and_then(|s| s.to_str()).unwrap()).unwrap());
            indexed.push(json!({"uri": uri(&path)?, "kind": SOURCE_FILE_KIND, "generated": path.starts_with(root.join(".build"))}));
        }
        let identifier = format!(
            "wisent-native:///{}",
            hex::encode(Sha256::digest(name.as_bytes()))
        );
        targets.push(json!({"id": {"uri": identifier}, "displayName": name, "languageIds": languages,
            "arguments": &arguments[1..], "compiler": compiler, "workingDirectory": package, "sources": indexed}));
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
