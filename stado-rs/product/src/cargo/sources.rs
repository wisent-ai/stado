use crate::{
    common::{atomic_json, checked, Runtime},
    source,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub struct Sources {
    pub metadata: Value,
    pub overrides: Vec<String>,
    pub records: Vec<Value>,
}

pub fn manifest(runtime: &Runtime, requested: &Path) -> Result<(PathBuf, PathBuf, String)> {
    let absolute = crate::common::absolute(requested)?;
    let physical = absolute.canonicalize()?;
    if physical != absolute {
        bail!(
            "Cargo manifest is not a physical canonical path: {}",
            absolute.display()
        );
    }
    if physical.file_name().and_then(|s| s.to_str()) != Some("Cargo.toml") || !physical.is_file() {
        bail!("Cargo manifest is absent: {}", physical.display());
    }
    let root = PathBuf::from(source::git(
        physical.parent().unwrap(),
        &["rev-parse", "--show-toplevel"],
    )?);
    let identity = source::repository(&source::git(&root, &["remote", "get-url", "origin"])?)
        .context("Cargo source has no GitHub origin")?;
    if source::checkout(runtime, &identity)? != root {
        bail!(
            "Cargo manifest is not in the canonical checkout: {}",
            physical.display()
        );
    }
    Ok((physical, root, identity))
}

fn package(root: &Path, name: &str) -> Result<PathBuf> {
    let output = checked(
        Command::new("git")
            .args([
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                "Cargo.toml",
                "**/Cargo.toml",
            ])
            .current_dir(root),
    )?;
    let paths = String::from_utf8(output.stdout)?;
    let mut matches = BTreeSet::new();
    for relative in paths.split('\0').filter(|s| !s.is_empty()) {
        let path = root.join(relative);
        let document: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
        if document
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
            == Some(name)
        {
            matches.insert(path);
        }
    }
    if matches.len() != 1 {
        bail!(
            "canonical Cargo package {name} has {} manifests in {}",
            matches.len(),
            root.display()
        );
    }
    Ok(matches.into_iter().next().unwrap())
}

struct Resolver<'a> {
    runtime: &'a Runtime,
    visited: BTreeSet<PathBuf>,
    roots: BTreeMap<String, PathBuf>,
    patches: BTreeMap<(String, String), PathBuf>,
    metadata: Option<Value>,
}

impl Resolver<'_> {
    fn visit(&mut self, candidate: &Path) -> Result<()> {
        if self.visited.contains(candidate) {
            return Ok(());
        }
        let (path, root, identity) = manifest(self.runtime, candidate)?;
        if !self.visited.insert(path.clone()) {
            return Ok(());
        }
        self.roots.insert(identity, root);
        let output = checked(
            Command::new("cargo")
                .args(["metadata", "--manifest-path"])
                .arg(&path)
                .args(["--format-version", "1", "--no-deps", "--offline"])
                .env("GIT_ALLOW_PROTOCOL", "")
                .current_dir(path.parent().unwrap()),
        )?;
        let metadata: Value = serde_json::from_slice(&output.stdout)?;
        if self.metadata.is_none() {
            self.metadata = Some(metadata.clone());
        }
        for member in metadata["packages"]
            .as_array()
            .context("Cargo metadata has no packages")?
        {
            let member_path = Path::new(
                member["manifest_path"]
                    .as_str()
                    .context("Cargo package has no manifest path")?,
            );
            manifest(self.runtime, member_path)?;
            self.visited.insert(member_path.to_path_buf());
            for dependency in member["dependencies"]
                .as_array()
                .context("Cargo package has no dependencies")?
            {
                let origin = dependency["source"].as_str();
                if let Some(remote) = origin.and_then(|s| s.strip_prefix("git+")) {
                    let mut url = url::Url::parse(remote)?;
                    url.set_query(None);
                    url.set_fragment(None);
                    let remote = url.to_string();
                    let identity = source::repository(&remote).with_context(|| {
                        format!("no canonical GitHub identity for Cargo dependency {remote}")
                    })?;
                    let root = if let Some(root) = self.roots.get(&identity) {
                        root.clone()
                    } else {
                        source::checkout(self.runtime, &identity)?
                    };
                    let name = dependency["name"]
                        .as_str()
                        .context("Cargo dependency has no package name")?;
                    let target = package(&root, name)?;
                    let key = (remote, name.to_owned());
                    let directory = target.parent().unwrap().to_path_buf();
                    if self.patches.get(&key).is_some_and(|old| old != &directory) {
                        bail!("conflicting canonical Cargo package {}", key.1);
                    }
                    self.patches.insert(key, directory);
                    self.visit(&target)?;
                } else if let Some(path) = dependency["path"].as_str() {
                    self.visit(&Path::new(path).join("Cargo.toml"))?;
                } else if origin
                    .is_some_and(|s| !s.starts_with("registry+") && !s.starts_with("sparse+"))
                {
                    bail!("unsupported Cargo dependency source: {}", origin.unwrap());
                }
            }
        }
        Ok(())
    }
}

pub fn prepare(runtime: &Runtime, path: &Path, evidence: &Path, scratch: &Path) -> Result<Sources> {
    let mut resolver = Resolver {
        runtime,
        visited: BTreeSet::new(),
        roots: BTreeMap::new(),
        patches: BTreeMap::new(),
        metadata: None,
    };
    resolver.visit(path)?;
    let mut overrides = Vec::new();
    for ((remote, name), directory) in &resolver.patches {
        overrides.push("--config".to_owned());
        overrides.push(format!(
            "patch.{}.{}.path={}",
            serde_json::to_string(remote)?,
            serde_json::to_string(name)?,
            serde_json::to_string(directory)?
        ));
    }
    let mut records = Vec::new();
    for (index, root) in resolver.roots.values().enumerate() {
        records.push(source::snapshot(
            root,
            &evidence.join("sources").join(index.to_string()),
            scratch,
        )?);
    }
    atomic_json(
        &evidence.join("sources.json"),
        &serde_json::to_value(&records)?,
    )?;
    Ok(Sources {
        metadata: resolver.metadata.context("Cargo source graph is empty")?,
        overrides,
        records,
    })
}
