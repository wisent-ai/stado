use crate::{
    common::{atomic_json, checked, toolchain_command, Runtime},
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

/// One repository's committed revision, exported as plain files, standing
/// in for its canonical checkout: every manifest under `exported` belongs to
/// `identity` at `commit`, whatever the checkout's working tree holds now.
#[derive(Debug, Clone)]
pub struct Pinned {
    pub identity: String,
    pub checkout: PathBuf,
    pub exported: PathBuf,
    pub commit: String,
    pub archive_sha256: String,
}

impl Pinned {
    /// The exported tree's record, in the place a checkout's snapshot
    /// takes: the commit and the digest of the archive it was unpacked from.
    fn record(&self) -> Value {
        serde_json::json!({
            "repository": self.identity,
            "repository_path": self.checkout,
            "exported_path": self.exported,
            "commit": self.commit,
            "archive_sha256": self.archive_sha256,
            "pinned": true,
        })
    }
}

/// Export `commit` of the canonical checkout `root` into the checkout's build
/// area under Stado's home, `stado-build/<commit>/`, once: a tree already
/// there for the same commit is kept, as its marker says. The tree holds
/// only what the commit holds, so a build of it reads no session's edit. It
/// is never written into the checkout, which sits in `~/Documents` where no
/// janitor pass reaches; `stado-build` is tagged `CACHEDIR.TAG`, because
/// every export in it is reproduced from its commit.
pub fn pin(runtime: &Runtime, requested: &Path, commit: &str) -> Result<Pinned> {
    let (manifest, root, identity) = manifest(runtime, requested)?;
    let full = source::git(
        &root,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .with_context(|| {
        format!(
            "{} does not hold commit {commit}; fetch or commit it first",
            root.display()
        )
    })?;
    let exports = crate::common::runs::checkout_area(&runtime.home, &root)?.join("stado-build");
    crate::common::tag_cache(&exports, "stado product cargo --revision")?;
    let exported = exports.join(&full);
    let marker = exported.join(".stado-exported.json");
    let archive_sha256 = match fs::read(&marker) {
        Ok(bytes) => {
            let kept: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("reading {}", marker.display()))?;
            kept["archive_sha256"]
                .as_str()
                .with_context(|| format!("{} names no archive digest", marker.display()))?
                .to_owned()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if exported.exists() {
                fs::remove_dir_all(&exported)
                    .with_context(|| format!("clearing {}", exported.display()))?;
            }
            let digest = source::export(&root, &full, &exported)?;
            crate::common::atomic_json(
                &marker,
                &serde_json::json!({"commit": full, "archive_sha256": digest}),
            )?;
            digest
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", marker.display())),
    };
    let relative = manifest
        .strip_prefix(&root)
        .with_context(|| format!("{} is not under {}", manifest.display(), root.display()))?;
    if !exported.join(relative).is_file() {
        bail!(
            "commit {full} of {identity} holds no {}; the manifest is not committed there",
            relative.display()
        );
    }
    Ok(Pinned {
        identity,
        checkout: root,
        exported,
        commit: full,
        archive_sha256,
    })
}

/// The manifest at `requested` as (its physical path, the root of the tree
/// it belongs to, that tree's repository identity). A manifest inside a
/// pinned export belongs to the pinned repository at its commit; every other
/// manifest must sit in its canonical checkout.
pub fn manifest_in(
    runtime: &Runtime,
    pinned: Option<&Pinned>,
    requested: &Path,
) -> Result<(PathBuf, PathBuf, String)> {
    if let Some(pinned) = pinned {
        let absolute = crate::common::absolute(requested)?;
        if absolute.starts_with(&pinned.exported) {
            if absolute.file_name().and_then(|s| s.to_str()) != Some("Cargo.toml")
                || !absolute.is_file()
            {
                bail!("Cargo manifest is absent: {}", absolute.display());
            }
            return Ok((absolute, pinned.exported.clone(), pinned.identity.clone()));
        }
    }
    manifest(runtime, requested)
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
    let identity =
        source::repository(&source::origin(&root)?).context("Cargo source has no GitHub origin")?;
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
    pinned: Option<&'a Pinned>,
    visited: BTreeSet<PathBuf>,
    roots: BTreeMap<String, PathBuf>,
    patches: BTreeMap<(String, String), PathBuf>,
    metadata: Option<Value>,
}

impl Resolver<'_> {
    /// `name`'s package directory under `root`: in the checkout's tracked
    /// files, re-rooted into the export when `root` is the pinned tree.
    fn package_under(&self, root: &Path, name: &str) -> Result<PathBuf> {
        match self.pinned.filter(|pinned| pinned.exported == root) {
            Some(pinned) => {
                let in_checkout = package(&pinned.checkout, name)?;
                let relative = in_checkout
                    .strip_prefix(&pinned.checkout)
                    .with_context(|| {
                        format!(
                            "{} is not under {}",
                            in_checkout.display(),
                            pinned.checkout.display()
                        )
                    })?;
                Ok(pinned.exported.join(relative))
            }
            None => package(root, name),
        }
    }

    fn visit(&mut self, candidate: &Path) -> Result<()> {
        if self.visited.contains(candidate) {
            return Ok(());
        }
        let (path, root, identity) = manifest_in(self.runtime, self.pinned, candidate)?;
        if !self.visited.insert(path.clone()) {
            return Ok(());
        }
        self.roots.insert(identity, root);
        let output = checked(
            toolchain_command("cargo")
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
            manifest_in(self.runtime, self.pinned, member_path)?;
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
                        source::required_checkout(self.runtime, &identity)?
                    };
                    let name = dependency["name"]
                        .as_str()
                        .context("Cargo dependency has no package name")?;
                    let target = self.package_under(&root, name)?;
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

pub fn prepare(
    runtime: &Runtime,
    pinned: Option<&Pinned>,
    path: &Path,
    evidence: &Path,
    scratch: &Path,
) -> Result<Sources> {
    let mut resolver = Resolver {
        runtime,
        pinned,
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
        // The pinned export is a committed tree, not a checkout: its record
        // is the commit and the archive digest, and nothing in it can be a
        // working-tree change.
        let record = match pinned.filter(|pinned| &pinned.exported == root) {
            Some(pinned) => {
                let record = pinned.record();
                let place = evidence.join("sources").join(index.to_string());
                fs::create_dir_all(&place)?;
                atomic_json(&place.join("source.json"), &record)?;
                record
            }
            None => source::snapshot(
                root,
                &evidence.join("sources").join(index.to_string()),
                scratch,
            )?,
        };
        records.push(record);
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
