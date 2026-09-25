use super::source::{self, PackageSource};
use crate::{
    common::{atomic_json, atomic_write, checked, xml, Runtime},
    source as git_source,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const WORKSPACE_ENV: &str = "WISENT_SWIFT_WORKSPACE";
pub const SCRATCH_ENV: &str = "SWIFTPM_BUILD_DIR";

fn single<'a>(value: &'a Value, label: &str) -> Result<&'a Value> {
    let array = value
        .as_array()
        .with_context(|| format!("Swift returned an unsupported {label}: {value}"))?;
    if array.len() != 1 || !array[0].is_object() {
        bail!("Swift returned an unsupported {label}: {value}");
    }
    Ok(&array[0])
}

struct Graph<'a> {
    runtime: &'a Runtime,
    scratch: &'a Path,
    packages: BTreeMap<PathBuf, Value>,
    mirrors: BTreeMap<String, PathBuf>,
}

impl Graph<'_> {
    fn visit(&mut self, selected: PackageSource) -> Result<()> {
        if self.packages.contains_key(&selected.path) {
            return Ok(());
        }
        let scratch = self.scratch.join(self.packages.len().to_string());
        let output = checked(
            Command::new("swift")
                .args(["package", "--package-path"])
                .arg(&selected.path)
                .arg("--scratch-path")
                .arg(&scratch)
                .args(["--disable-keychain", "dump-package"])
                .current_dir(&selected.path)
                .env("TMPDIR", self.scratch)
                .env(SCRATCH_ENV, &scratch)
                .env("GIT_ALLOW_PROTOCOL", ""),
        )?;
        let manifest: Value = serde_json::from_slice(&output.stdout)?;
        let dependencies = manifest["dependencies"].as_array().with_context(|| {
            format!(
                "Swift returned no dependency array for {}",
                selected.path.display()
            )
        })?;
        let name = manifest["name"]
            .as_str()
            .context("Swift package manifest has no name")?;
        self.packages
            .insert(selected.path.clone(), selected.record(name));
        for dependency in dependencies {
            let target = if let Some(control) = dependency.get("sourceControl") {
                let control = single(control, "source-control dependency")?;
                let remote = single(&control["location"]["remote"], "remote source location")?;
                let url = remote["urlString"]
                    .as_str()
                    .context("Swift source dependency has no URL")?;
                let identity = git_source::repository(url)
                    .with_context(|| format!("no canonical GitHub source identity for {url}"))?;
                let target = git_source::checkout(self.runtime, &identity)?;
                self.mirrors.insert(url.to_owned(), target.clone());
                target
            } else if let Some(filesystem) = dependency.get("fileSystem") {
                let filesystem = single(filesystem, "filesystem dependency")?;
                let target = PathBuf::from(
                    filesystem["path"]
                        .as_str()
                        .context("Swift filesystem dependency has no path")?,
                );
                if target.is_absolute() {
                    target
                } else {
                    selected.path.join(target)
                }
            } else {
                bail!("unsupported Swift dependency source: {dependency}");
            };
            let dependency = source::package(self.runtime, &target)
                .with_context(|| format!("native dependency of {}", selected.path.display()))?;
            self.visit(dependency)?;
        }
        Ok(())
    }
}

pub fn prepare(
    runtime: &Runtime,
    root: &PackageSource,
    workspace: &Path,
    evidence: &Path,
) -> Result<Vec<Value>> {
    let parent = workspace
        .parent()
        .context("Swift workspace has no parent")?;
    fs::create_dir_all(parent)?;
    let manifests = parent.join(format!("manifests-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&manifests)?;
    let mut graph = Graph {
        runtime,
        scratch: &manifests,
        packages: BTreeMap::new(),
        mirrors: BTreeMap::new(),
    };
    let resolved = graph.visit(root.clone());
    fs::remove_dir_all(&manifests)?;
    resolved?;
    fs::create_dir(workspace)?;
    let mut document =
        String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Workspace version=\"1.0\">\n");
    for path in graph.packages.keys() {
        document.push_str(&format!(
            "<FileRef location=\"absolute:{}\"/>\n",
            xml(&path.to_string_lossy())
        ));
    }
    document.push_str("</Workspace>\n");
    atomic_write(
        &workspace.join("contents.xcworkspacedata"),
        document.as_bytes(),
    )?;
    let configuration = parent.join(format!("configuration-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&configuration)?;
    let configured = (|| -> Result<()> {
        for (original, target) in &graph.mirrors {
            let mirror = url::Url::from_directory_path(target).map_err(|_| {
                anyhow::anyhow!("cannot encode canonical Swift mirror {}", target.display())
            })?;
            checked(
                Command::new("swift")
                    .args(["package", "--package-path"])
                    .arg(&root.path)
                    .arg("--scratch-path")
                    .arg(&configuration)
                    .arg("--multiroot-data-file")
                    .arg(workspace)
                    .args([
                        "--disable-keychain",
                        "config",
                        "set-mirror",
                        "--original",
                        original,
                        "--mirror",
                        mirror.as_str(),
                    ])
                    .current_dir(&root.path)
                    .env("TMPDIR", &configuration)
                    .env(SCRATCH_ENV, &configuration)
                    .env("GIT_ALLOW_PROTOCOL", ""),
            )?;
        }
        Ok(())
    })();
    fs::remove_dir_all(&configuration)?;
    configured?;
    let mut recorded_roots = BTreeSet::new();
    for (index, record) in graph.packages.values_mut().enumerate() {
        let directory = evidence.join("sources").join(index.to_string());
        if let Some(repository) = record["repository_path"].as_str() {
            if recorded_roots.insert(repository.to_owned()) {
                record["source_snapshot"] =
                    git_source::snapshot(Path::new(repository), &directory, parent)?;
            }
        }
        atomic_json(&directory.join("package.json"), record)?;
    }
    Ok(graph.packages.into_values().collect())
}
