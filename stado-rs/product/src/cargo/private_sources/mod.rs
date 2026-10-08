//! Private Git packages are release inputs, never builder credentials.
mod execute;
mod export;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

pub(super) use execute::execute;
pub use export::export;

pub const INPUT_NAME: &str = "private-cargo-sources";
const INPUT_ENV: &str = "WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct Package {
    name: String,
    version: String,
    source: String,
}

/// The provenance layout `export` writes and `cargo` accepts. Published inputs
/// carry it, so it changes only together with a new input and both readers.
pub(super) const PROVENANCE_SCHEMA_VERSION: u64 = 1;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    schema_version: u64,
    cargo_lock_sha256: String,
    packages: BTreeSet<Package>,
}

fn locked_packages(root: &Path) -> Result<BTreeSet<Package>> {
    let path = root.join("Cargo.lock");
    let text = fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let document: toml::Value =
        toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))?;
    let packages = document
        .get("package")
        .and_then(toml::Value::as_array)
        .with_context(|| format!("{} has no package array", path.display()))?;
    let mut locked = BTreeSet::new();
    for package in packages {
        let Some(source) = package.get("source").and_then(toml::Value::as_str) else {
            continue;
        };
        if !source.starts_with("git+") {
            continue;
        }
        let field = |name| {
            package
                .get(name)
                .and_then(toml::Value::as_str)
                .with_context(|| {
                    format!("{}: Git package has no {name}: {package}", path.display())
                })
        };
        locked.insert(Package {
            name: field("name")?.to_owned(),
            version: field("version")?.to_owned(),
            source: source.to_owned(),
        });
    }
    Ok(locked)
}

fn source_config(packages: &BTreeSet<Package>) -> Result<String> {
    let sources: BTreeSet<_> = packages.iter().map(|package| &package.source).collect();
    let quoted = |value: &str| serde_json::Value::String(value.into()).to_string();
    let mut lines = Vec::new();
    for (index, source) in sources.into_iter().enumerate() {
        let located = source
            .strip_prefix("git+")
            .context("expected a Git source")?;
        let mut url = url::Url::parse(located)
            .with_context(|| format!("unsupported locked Git source: {source}"))?;
        let mut references = BTreeMap::<String, String>::new();
        for (key, value) in url.query_pairs() {
            if !matches!(key.as_ref(), "branch" | "tag" | "rev")
                || references
                    .insert(key.into_owned(), value.into_owned())
                    .is_some()
            {
                bail!("unsupported or ambiguous locked Git reference: {source}");
            }
        }
        url.set_query(None);
        url.set_fragment(None);
        lines.push(format!("[source.private-git-{index}]"));
        lines.push(format!("git = {}", quoted(url.as_str())));
        for (key, value) in references {
            lines.push(format!("{key} = {}", quoted(&value)));
        }
        lines.push(format!("replace-with = {}", quoted(INPUT_NAME)));
        lines.push(String::new());
    }
    lines.push(format!("[source.{INPUT_NAME}]"));
    lines.push("directory = \"sources\"".into());
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn configure(command: &mut std::process::Command, root: &Path) -> Result<()> {
    let locked = locked_packages(root)?;
    if locked.is_empty() {
        return Ok(());
    }
    let configured = std::env::var_os(INPUT_ENV)
        .with_context(|| format!("{INPUT_ENV} is required: publish the locked Git crates with stado release catalog pin-input --cargo"))?;
    let input = fs::canonicalize(&configured)
        .with_context(|| format!("cannot read {INPUT_ENV}={configured:?}"))?;
    let path = input.join("provenance.json");
    let provenance: Provenance = serde_json::from_slice(&fs::read(&path)?).with_context(|| {
        format!(
            "cannot read private Cargo source provenance {}",
            path.display()
        )
    })?;
    if provenance.schema_version != PROVENANCE_SCHEMA_VERSION {
        bail!(
            "unsupported private Cargo source schema in {}",
            path.display()
        );
    }
    if provenance.packages != locked {
        bail!("private Cargo sources do not match Cargo.lock: input={:?}; locked={locked:?}; publish a new input", provenance.packages);
    }
    let configuration = input.join("config.toml");
    if fs::read_to_string(&configuration)? != source_config(&locked)? {
        bail!(
            "{} disagrees with private Cargo source provenance",
            configuration.display()
        );
    }
    command
        .arg("--config")
        .arg(configuration)
        .arg("--config")
        .arg(format!(
            "source.{INPUT_NAME}.directory={}",
            serde_json::to_string(&input.join("sources"))?
        ));
    Ok(())
}
