//! Cross-product CLI verbs promised by revisioned sidecars and qualified by CI.
//! The first provider revision declaring a verb is the minimum source
//! revision; no moving branch tip or handwritten commit pin defines it.

use crate::{
    catalog,
    common::Runtime,
    source,
    state::{InstallStatus, ProductState},
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
mod chain;
mod contract;
pub use chain::chain;
use contract::{first_provider, manifest, provides};

const CI_RECORD: &str = "ci-main.json";
const CI_RECORD_ENV: &str = "STADO_CI_MAIN_RECORD";
/// The installations in progress a parent process hands to the binary that
/// finishes one of their dependencies (`stado product finish`), as JSON.
pub const ENCLOSING_ENV: &str = "STADO_PRODUCT_ENCLOSING";

#[derive(Clone, Deserialize)]
pub struct Requirement {
    pub product: String,
    pub surface: String,
    pub verb: String,
}

/// An installation in progress that the one being planned is a dependency
/// of: the product and surface it installs and the revision it builds.
#[derive(Clone, Deserialize, Serialize)]
pub struct Enclosing {
    pub product: String,
    pub surface: String,
    pub revision: String,
}

impl Enclosing {
    pub fn new(product: &str, surface: &str, revision: &str) -> Self {
        Self {
            product: product.to_owned(),
            surface: surface.to_owned(),
            revision: revision.to_owned(),
        }
    }

    pub fn node(&self) -> String {
        format!("{}/{}", self.product, self.surface)
    }
}

/// The enclosing installations this process was handed in `ENCLOSING_ENV`;
/// none when it was handed none, and a value that is not that list is
/// refused naming the variable.
fn handed() -> Result<Vec<Enclosing>> {
    match std::env::var(ENCLOSING_ENV) {
        Err(std::env::VarError::NotPresent) => Ok(Vec::new()),
        Err(error) => bail!("{ENCLOSING_ENV} cannot be read: {error}"),
        Ok(text) => serde_json::from_str(&text).with_context(|| {
            format!("{ENCLOSING_ENV} holds no list of enclosing installations: {text}")
        }),
    }
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
        .with_context(|| format!("{key} has no qualified last-green revision in {}; `stado ci delivery` names each step's last checked and last green commit and why", path.display()))
}

/// What each verb `product` requires at `revision` needs. A verb whose
/// provider is an installation `enclosing` (or a parent process handed)
/// names, and whose target revision declares it, is met by that revision:
/// the enclosing installation places it once this dependency is done, so a
/// product whose own dependency needs its new verb can still be installed.
pub fn requirements(
    runtime: &Runtime,
    document: &Value,
    product: &Value,
    surface: &str,
    revision: &str,
    enclosing: &[Enclosing],
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
    let handed = handed()?;
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
        let installed = ProductState::load(runtime, &required.product, &required.surface)?;
        let source_revision = installed
            .as_ref()
            .and_then(|state| state.source_revision.clone());
        let in_progress = enclosing
            .iter()
            .chain(handed.iter())
            .find(|frame| frame.product == required.product && frame.surface == required.surface);
        if let Some(frame) = in_progress {
            let promised = manifest(&root, &frame.revision, provider_path)?;
            if promised.product == required.product && provides(&promised, &required.verb) {
                let minimum =
                    first_provider(&root, &frame.revision, provider_path, &required.verb)?;
                let reason = format!(
                    "the installation of {} at {} this one is a dependency of provides the verb and places it once this one is done",
                    frame.node(),
                    frame.revision
                );
                decisions.push(Decision {
                    requirement: required,
                    minimum,
                    green: frame.revision.clone(),
                    installed: source_revision,
                    satisfied: true,
                    reason,
                });
                continue;
            }
        }
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
        let installed_text = match source_revision.as_deref() {
            Some(revision) => format!("installed revision {revision}"),
            None => "not installed".to_owned(),
        };
        let qualified = if satisfied {
            source_revision
                .clone()
                .context("installed prerequisite has no source revision")?
        } else {
            green(runtime, &required.product, &required.surface).with_context(|| {
                format!(
                    "{} {} needs verb {}; {installed_text}; a pass of the delivery routine (`stado ci run`, with the provider's step in `stado ci plan`) qualifies a provider revision",
                    required.product, required.surface, required.verb
                )
            })?
        };
        let promised = manifest(&root, &qualified, provider_path)?;
        if promised.product != required.product || !provides(&promised, &required.verb) {
            bail!(
                "{} {} needs verb {}; qualified last green {qualified} does not provide it; {installed_text}",
                required.product, required.surface, required.verb
            );
        }
        let minimum = first_provider(&root, &qualified, provider_path, &required.verb)?;
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
