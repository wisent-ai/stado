//! Cross-product CLI verbs promised by source manifests and qualified by CI.
//! The provider's first manifest revision declaring a verb is the minimum
//! source revision; no moving branch tip or handwritten commit pin defines it.

use crate::{
    catalog,
    common::Runtime,
    source,
    state::{InstallStatus, ProductState},
};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
mod chain;
pub use chain::chain;

const CI_RECORD: &str = "ci-main.json";
const CI_RECORD_ENV: &str = "STADO_CI_MAIN_RECORD";

#[derive(Deserialize)]
struct Manifest {
    product: String,
    #[serde(default)]
    provides: Vec<Provided>,
    #[serde(default)]
    requires: Vec<Requirement>,
}

#[derive(Deserialize)]
struct Provided {
    verb: String,
}

#[derive(Clone, Deserialize)]
pub struct Requirement {
    pub product: String,
    pub surface: String,
    pub verb: String,
}

pub struct Decision {
    pub requirement: Requirement,
    pub minimum: String,
    pub green: String,
    pub installed: Option<String>,
    pub satisfied: bool,
    pub reason: String,
}

impl Decision {
    pub fn report(&self) -> Value {
        json!({
            "product": self.requirement.product,
            "surface": self.requirement.surface,
            "verb": self.requirement.verb,
            "minimum_revision": self.minimum,
            "revision": self.green,
            "installed_revision": self.installed,
            "action": if self.satisfied { "reuse" } else { "install" },
            "reason": self.reason,
        })
    }

    pub fn refusal(&self) -> String {
        let installed = match self.installed.as_deref() {
            Some(revision) => format!("installed revision {revision}"),
            None => "it is not installed".to_owned(),
        };
        format!(
            "{} {} needs verb {} from revision {}; {installed} ({}); qualified last green {}; use --with-prerequisites to install it first, or --plan to inspect the chain",
            self.requirement.product, self.requirement.surface,
            self.requirement.verb, self.minimum, self.reason, self.green,
        )
    }
}

fn checkout(runtime: &Runtime, product: &Value, surface: &str) -> Result<PathBuf> {
    let selected = crate::install::recipe(product, surface)?;
    let repository = selected["repository"]
        .as_str()
        .or_else(|| product["repository"].as_str())
        .context("product installation has no repository")?;
    source::required_checkout(runtime, repository)
}

fn manifest_path<'a>(product: &'a Value, surface: &str) -> Result<Option<&'a str>> {
    Ok(crate::install::recipe(product, surface)?["manifest"].as_str())
}

pub fn revision(
    runtime: &Runtime,
    product: &Value,
    surface: &str,
    coordinate: Option<crate::install::Coordinate<'_>>,
) -> Result<String> {
    match coordinate {
        Some(coordinate) => Ok(coordinate.revision().to_owned()),
        None => source::git(
            &checkout(runtime, product, surface)?,
            &["rev-parse", "HEAD"],
        ),
    }
}

fn manifest(root: &std::path::Path, revision: &str, path: &str) -> Result<Manifest> {
    let reference = format!("{revision}:{path}");
    let content = source::git(root, &["show", &reference])
        .with_context(|| format!("reading release manifest {path} at {revision}"))?;
    serde_json::from_str(&content)
        .with_context(|| format!("parsing release manifest {path} at {revision}"))
}

fn provides(manifest: &Manifest, verb: &str) -> bool {
    manifest
        .provides
        .iter()
        .any(|provided| provided.verb == verb)
}

fn first_provider(
    root: &std::path::Path,
    revision: &str,
    path: &str,
    verb: &str,
) -> Result<String> {
    let pickaxe = format!("-S{verb}");
    let history = source::git(
        root,
        &[
            "log",
            "--reverse",
            "--format=%H",
            &pickaxe,
            revision,
            "--",
            path,
        ],
    )?;
    for candidate in history.lines() {
        let declared = manifest(root, candidate, path)?;
        if provides(&declared, verb) {
            return Ok(candidate.to_owned());
        }
    }
    bail!("provider has no source revision declaring the verb {verb} at {revision}")
}

fn green(runtime: &Runtime, product: &str, surface: &str) -> Result<String> {
    let path = match std::env::var_os(CI_RECORD_ENV) {
        Some(path) => PathBuf::from(path),
        None => runtime.home.join(".stado").join(CI_RECORD),
    };
    let record: Value = serde_json::from_slice(
        &fs::read(&path)
            .with_context(|| format!("reading CI main check record {}", path.display()))?,
    )?;
    let key = format!("{product}:{surface}");
    record["steps"][&key].as_array()
        .and_then(|runs| runs.iter().rev().find(|run| run["green"] == true))
        .and_then(|run| run["revision"].as_str())
        .map(str::to_owned)
        .with_context(|| format!("{key} has no qualified last-green revision in {}; stado ci main history --product {product} names why", path.display()))
}

pub fn requirements(
    runtime: &Runtime,
    document: &Value,
    product: &Value,
    surface: &str,
    revision: &str,
) -> Result<Vec<Decision>> {
    let id = catalog::text(product, "id")?;
    let Some(path) = manifest_path(product, surface)? else {
        return Ok(Vec::new());
    };
    let root = checkout(runtime, product, surface)?;
    let own = manifest(&root, revision, path)?;
    if own.product != id {
        bail!(
            "release manifest {path} at {revision} declares {} instead of {id}",
            own.product
        );
    }
    let mut decisions = Vec::new();
    for required in own.requires {
        let provider = catalog::product(document, &required.product)?;
        if required.product == id {
            bail!(
                "{id} requires its own verb {} as an external prerequisite",
                required.verb
            );
        }
        let provider_path = manifest_path(provider, &required.surface)?.with_context(|| {
            format!(
                "{} {} has no release manifest declaring verb {}",
                required.product, required.surface, required.verb
            )
        })?;
        let root = checkout(runtime, provider, &required.surface)?;
        let provider_head = source::git(&root, &["rev-parse", "HEAD"])?;
        let available = manifest(&root, &provider_head, provider_path)?;
        if available.product != required.product || !provides(&available, &required.verb) {
            bail!(
                "{} {} at {provider_head} does not declare verb {}; no providing revision is known",
                required.product,
                required.surface,
                required.verb
            );
        }
        let minimum = first_provider(&root, &provider_head, provider_path, &required.verb)?;
        let installed = ProductState::load(runtime, &required.product, &required.surface)?;
        let source_revision = installed
            .as_ref()
            .and_then(|state| state.source_revision.clone());
        let (satisfied, reason) = match (installed.as_ref(), source_revision.as_deref()) {
            (Some(state), Some(revision)) if state.status == InstallStatus::Installed => {
                match manifest(&root, revision, provider_path) {
                    Ok(declared)
                        if declared.product == required.product
                            && provides(&declared, &required.verb) =>
                    {
                        match crate::install::transaction::ownership::verify(state) {
                            Ok(()) => (
                                true,
                                "installed revision provides the verb and its files verify"
                                    .to_owned(),
                            ),
                            Err(error) => (
                                false,
                                format!("installed files failed verification: {error:#}"),
                            ),
                        }
                    }
                    Ok(_) => (
                        false,
                        "installed revision does not declare the verb".to_owned(),
                    ),
                    Err(error) => (
                        false,
                        format!("installed revision manifest cannot be read: {error:#}"),
                    ),
                }
            }
            (Some(state), _) => (
                false,
                format!(
                    "installed state is {:?} or has no source revision",
                    state.status
                ),
            ),
            (None, _) => (false, "not installed".to_owned()),
        };
        let qualified = if satisfied {
            source_revision
                .clone()
                .context("installed prerequisite has no source revision")?
        } else {
            green(runtime, &required.product, &required.surface).with_context(|| {
                format!(
                    "{} {} needs verb {} from revision {}; installed revision {:?}",
                    required.product, required.surface, required.verb, minimum, source_revision
                )
            })?
        };
        let promised = manifest(&root, &qualified, provider_path)?;
        if promised.product != required.product || !provides(&promised, &required.verb) {
            bail!("{} {} needs verb {} from revision {}; qualified last green {qualified} does not provide it; installed revision {:?}",
                  required.product, required.surface, required.verb, minimum, source_revision);
        }
        decisions.push(Decision {
            requirement: required,
            minimum,
            green: qualified,
            installed: source_revision,
            satisfied,
            reason,
        });
    }
    Ok(decisions)
}
