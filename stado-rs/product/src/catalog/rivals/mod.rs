//! A product's `rivals` and the `benchmark` that measures it against them.
//!
//! Naming a rival is a claim about the market; the benchmark is the check
//! that confronts it. So the two are declared together or not at all: a
//! rival with no benchmark is a promise nothing measures, and a benchmark
//! with no rival measures the product against nobody. `benchmark.app` is the
//! Probierz application whose manifest declares the suites, and every rival
//! id is the id of the contender that runs it there. A rival may carry
//! `claims`, each statement bound to the page it was read from and its
//! digest ([`claims`]), so a comparison names its sources and a changed
//! source invalidates only the claim that rests on it.

use crate::common::slug;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

mod claims;
pub use claims::check as check_claims;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rival {
    id: String,
    name: String,
    url: String,
    evidence: String,
    #[serde(default)]
    claims: Vec<claims::Claim>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Benchmark {
    app: String,
    suites: Vec<String>,
}

pub(super) fn validate(id: &str, product: &Value) -> Result<()> {
    let rivals = match product.get("rivals") {
        None => Vec::new(),
        Some(value) => Vec::<Rival>::deserialize(value).with_context(|| {
            format!("{id}.rivals: expected a list of {{id, name, url, evidence, claims?}}")
        })?,
    };
    let mut seen = HashSet::new();
    for rival in &rivals {
        slug(&rival.id).with_context(|| format!("{id}.rivals: {} is not a slug", rival.id))?;
        if rival.id == id {
            bail!("{id}.rivals: a product is not its own rival");
        }
        if !seen.insert(rival.id.as_str()) {
            bail!("{id}.rivals: {} is declared twice", rival.id);
        }
        if rival.name.trim().is_empty() || rival.evidence.trim().is_empty() {
            bail!(
                "{id}.rivals.{}: name and evidence must be non-empty",
                rival.id
            );
        }
        if !rival.url.starts_with("https://") {
            bail!(
                "{id}.rivals.{}: url must be https, got {}",
                rival.id,
                rival.url
            );
        }
        claims::validate(id, &rival.id, &rival.claims)?;
    }
    let benchmark = match product.get("benchmark") {
        None => None,
        Some(value) => Some(
            Benchmark::deserialize(value)
                .with_context(|| format!("{id}.benchmark: expected {{app, suites}}"))?,
        ),
    };
    match (&benchmark, rivals.is_empty()) {
        (None, false) => bail!(
            "{id}.rivals: declared rivals need a benchmark that measures them; add benchmark: {{app, suites}}"
        ),
        (Some(_), true) => {
            bail!("{id}.benchmark: a benchmark needs at least one declared rival to measure against")
        }
        _ => {}
    }
    if let Some(benchmark) = benchmark {
        slug(&benchmark.app)
            .with_context(|| format!("{id}.benchmark.app: {} is not a slug", benchmark.app))?;
        if benchmark.suites.is_empty() {
            bail!("{id}.benchmark.suites: at least one suite is required");
        }
        for suite in &benchmark.suites {
            slug(suite).with_context(|| format!("{id}.benchmark.suites: {suite} is not a slug"))?;
        }
    }
    Ok(())
}
