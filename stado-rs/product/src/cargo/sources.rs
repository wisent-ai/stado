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

use super::pinned::Pinned;

pub struct Sources {
    pub metadata: Value,
    pub overrides: Vec<String>,
    pub records: Vec<Value>,
    /// Every committed tree the build reads instead of a checkout: the
    /// pinned revision, and each Git dependency built from its pin.
    pub exports: Vec<Pinned>,
}

/// The manifest at `requested` as (its physical path, the root of the tree
/// it belongs to, that tree's repository identity). A manifest inside one of
/// the `exports` belongs to that repository at its commit; every other
/// manifest must sit in its canonical checkout.
pub fn manifest_in(
    runtime: &Runtime,
    exports: &[Pinned],
    requested: &Path,
) -> Result<(PathBuf, PathBuf, String)> {
    let absolute = crate::common::absolute(requested)?;
    if let Some(export) = exports
        .iter()
        .find(|export| absolute.starts_with(&export.exported))
    {
        if absolute.file_name().and_then(|s| s.to_str()) != Some("Cargo.toml")
            || !absolute.is_file()
        {
            bail!("Cargo manifest is absent: {}", absolute.display());
        }
        return Ok((absolute, export.exported.clone(), export.identity.clone()));
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

/// Whether `root`, the canonical checkout of `identity` that replaces a Git
/// dependency, carries `rev`, the revision the manifest `by` pins: `Err`
/// naming the checkout, its `HEAD` and the pin when it does not, since a
/// build of it would compile the dependency older than the manifest was
/// written against and fail on items the pin added, with errors that name
/// the depending crate, not the checkout. A `HEAD` at or past the pin is
/// built as the checkout is.
fn carries(root: &Path, identity: &str, rev: &str, by: &str) -> Result<()> {
    let head = source::git(root, &["rev-parse", "HEAD"])?;
    let known = source::git(root, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]);
    let carried = known.is_ok()
        && Command::new("git")
            .args(["merge-base", "--is-ancestor", rev, &head])
            .current_dir(root)
            .status()
            .with_context(|| format!("git merge-base did not run in {}", root.display()))?
            .success();
    if carried {
        return Ok(());
    }
    let why = match known {
        Ok(_) => "is not carried by its HEAD",
        Err(_) => "is not in it at all (it was never fetched)",
    };
    bail!(
        "the canonical checkout {} of {identity} stands at {head}, and the revision {rev} that \
         {by} pins {why}: Stado builds a Git dependency from its canonical checkout, so this \
         build would compile {identity} older than its pin. Bring the checkout up to its origin \
         (`git -C {} pull --ff-only`), or build a committed revision with --revision, which \
         builds such a dependency from its pin",
        root.display(),
        root.display()
    )
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
    let count = matches.len();
    let mut found = matches.into_iter();
    match (found.next(), found.next()) {
        (Some(manifest), None) => Ok(manifest),
        _ => bail!(
            "canonical Cargo package {name} has {count} manifests in {}",
            root.display()
        ),
    }
}

struct Resolver<'a> {
    runtime: &'a Runtime,
    /// Whether the build reads a committed revision (`--revision`), so a
    /// dependency behind its pin is exported at the pin instead of refused.
    committed: bool,
    exports: Vec<Pinned>,
    visited: BTreeSet<PathBuf>,
    roots: BTreeMap<String, PathBuf>,
    patches: BTreeMap<(String, String), PathBuf>,
    metadata: Option<Value>,
}

impl Resolver<'_> {
    /// `name`'s package directory under `root`: in the checkout's tracked
    /// files, re-rooted into the export when `root` is an exported tree.
    fn package_under(&self, root: &Path, name: &str) -> Result<PathBuf> {
        match self.exports.iter().find(|export| export.exported == root) {
            Some(export) => {
                let in_checkout = package(&export.checkout, name)?;
                let relative = in_checkout
                    .strip_prefix(&export.checkout)
                    .with_context(|| {
                        format!(
                            "{} is not under {}",
                            in_checkout.display(),
                            export.checkout.display()
                        )
                    })?;
                Ok(export.exported.join(relative))
            }
            None => package(root, name),
        }
    }

    /// The tree a Git dependency of `identity` pinned at `rev` by the
    /// manifest `by` is built from. A build of a committed revision
    /// (`--revision`) compiles the pin itself, fetched into the canonical
    /// checkout `root` when it lacks it and exported like the revision being
    /// built: neither a checkout behind the pin nor a session's uncommitted
    /// edit in a checkout at the pin reaches it, so it compiles what its lock
    /// names. A build of the working tree compiles the checkout when its
    /// `HEAD` carries the pin, and is refused when it does not.
    fn pinned_tree(&mut self, root: PathBuf, identity: &str, rev: &str, by: &str) -> Result<PathBuf> {
        if !self.committed {
            carries(&root, identity, rev, by)?;
            return Ok(root);
        }
        source::hold(&root, rev).with_context(|| {
            format!("building {identity} at {rev}, the revision {by} pins")
        })?;
        let full = source::git(&root, &["rev-parse", "--verify", &format!("{rev}^{{commit}}")])?;
        let export = super::pinned::exported(self.runtime, root, identity.to_owned(), full)?;
        let tree = export.exported.clone();
        self.exports.push(export);
        Ok(tree)
    }

    fn visit(&mut self, candidate: &Path) -> Result<()> {
        if self.visited.contains(candidate) {
            return Ok(());
        }
        let (path, root, identity) = manifest_in(self.runtime, &self.exports, candidate)?;
        if !self.visited.insert(path.clone()) {
            return Ok(());
        }
        self.roots.insert(identity, root);
        let output = checked(
            toolchain_command("cargo")?
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
            manifest_in(self.runtime, &self.exports, member_path)?;
            self.visited.insert(member_path.to_path_buf());
            for dependency in member["dependencies"]
                .as_array()
                .context("Cargo package has no dependencies")?
            {
                let origin = dependency["source"].as_str();
                if let Some(remote) = origin.and_then(|s| s.strip_prefix("git+")) {
                    let mut url = url::Url::parse(remote)?;
                    let pinned_rev = url
                        .query_pairs()
                        .find(|(key, _)| key == "rev")
                        .map(|(_, rev)| rev.into_owned());
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
                    // An exported tree is one commit, not a checkout: the
                    // revision the build was asked for, or a pin already
                    // exported.
                    let exported = self.exports.iter().any(|export| export.exported == root);
                    let root = match (&pinned_rev, exported) {
                        (Some(rev), false) => {
                            let by = member_path.display().to_string();
                            self.pinned_tree(root, &identity, rev, &by)?
                        }
                        _ => root,
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
        committed: pinned.is_some(),
        exports: pinned.cloned().into_iter().collect(),
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
        // An exported tree is a committed tree, not a checkout: its record
        // is the commit and the archive digest, and nothing in it can be a
        // working-tree change.
        let record = match resolver.exports.iter().find(|export| &export.exported == root) {
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
        exports: resolver.exports,
    })
}
