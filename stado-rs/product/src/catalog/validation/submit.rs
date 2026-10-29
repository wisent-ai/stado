//! A product's `submit` declaration: how the submit queue (`stado change`)
//! classifies a change of it before the change lands.
//!
//! ```yaml
//! submit:
//!   criticality: standard          # or critical
//!   low_risk_max_changed_lines: 400  # standard only
//!   review_paths:
//!   - .wisent-release.json
//!   - .github/
//! ```
//!
//! Every change of a `critical` product is high risk. A change of a
//! `standard` product is high risk when it changes more lines than its
//! `low_risk_max_changed_lines` or touches a `review_paths` entry (the path
//! itself, or anything below an entry ending in `/`). A high-risk change is
//! reviewed before it lands; every other change lands first and is
//! reviewed after. The numbers and paths are the product's declaration; the
//! queue keeps none of its own.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

/// How much a product's change can hurt, as the catalog declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Criticality {
    /// Every change is reviewed before it lands.
    Critical,
    /// A change is reviewed before it lands only when its size or paths
    /// say so.
    Standard,
}

impl Criticality {
    pub fn word(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::Standard => "standard",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitPolicy {
    pub criticality: Criticality,
    /// Declared by a `standard` product only: a critical one reviews every
    /// change first, whatever its size.
    #[serde(default)]
    pub low_risk_max_changed_lines: Option<usize>,
    #[serde(default)]
    pub review_paths: Vec<String>,
}

impl SubmitPolicy {
    /// The declared review path `path` falls under, if any.
    pub fn review_path(&self, path: &str) -> Option<&str> {
        self.review_paths
            .iter()
            .find(|declared| {
                if declared.ends_with('/') {
                    path.starts_with(declared.as_str())
                } else {
                    path == declared.as_str()
                }
            })
            .map(String::as_str)
    }
}

/// `product`'s declaration, `None` when its record declares none.
pub fn policy(id: &str, product: &Value) -> Result<Option<SubmitPolicy>> {
    let Some(declared) = product.get("submit") else {
        return Ok(None);
    };
    let policy = SubmitPolicy::deserialize(declared)
        .with_context(|| format!("{id}: submit is not a submit declaration"))?;
    Ok(Some(policy))
}

pub(super) fn validate(id: &str, product: &Value) -> Result<()> {
    let Some(policy) = policy(id, product)? else {
        return Ok(());
    };
    match (policy.criticality, policy.low_risk_max_changed_lines) {
        (Criticality::Standard, None) => bail!(
            "{id}: submit declares criticality standard and no low_risk_max_changed_lines, so no \
             change of it can be told low risk; declare the most lines a change may change and \
             still land before its review"
        ),
        (Criticality::Standard, Some(0)) => bail!(
            "{id}: submit.low_risk_max_changed_lines is 0; a product whose every change is \
             reviewed first declares criticality: critical"
        ),
        (Criticality::Critical, Some(_)) => bail!(
            "{id}: submit declares criticality critical beside low_risk_max_changed_lines; every \
             change of a critical product is reviewed first, so remove the number"
        ),
        _ => {}
    }
    for path in &policy.review_paths {
        if path.trim().is_empty()
            || path.starts_with('/')
            || path.split('/').any(|part| part == "..")
        {
            bail!("{id}: submit.review_paths entry {path:?} is not a path inside the repository");
        }
    }
    Ok(())
}
