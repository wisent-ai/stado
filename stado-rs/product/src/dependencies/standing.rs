//! Whether a product the source reaches is one it can run without.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

use crate::catalog::Dependency;

/// What the catalog says about a product the code reaches, checked against
/// the workspace.
#[derive(Debug, Serialize)]
#[serde(tag = "standing", rename_all = "kebab-case")]
pub enum Standing {
    /// Every declared need of it has an alternative whose code is in the
    /// workspace.
    Independent {
        alternative: String,
        alternative_source: String,
    },
    /// The catalog declares no dependency on it, so nothing says what the
    /// product does when it is absent.
    Undeclared,
    /// A declared need of it has no alternative: the product does not run
    /// without it, by design when the catalog says why.
    Bound {
        surface: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        because: Option<String>,
    },
    /// An alternative is declared and no code is named for it.
    Unproven { alternative: String },
    /// The code named for the alternative is not a file in the workspace.
    SourceMissing { alternative_source: String },
}

impl Standing {
    /// The product runs without the other.
    pub fn independent(&self) -> bool {
        matches!(self, Standing::Independent { .. })
    }

    /// Why `product` cannot be shown to run without `other`; `None` when it
    /// can.
    pub fn refusal(&self, product: &str, other: &str) -> Option<String> {
        match self {
            Standing::Independent { .. } => None,
            Standing::Undeclared => Some(format!(
                "the catalog declares no dependency of {product} on {other}, so nothing says \
                 what {product} does without it"
            )),
            Standing::Bound {
                surface,
                because: Some(because),
            } => Some(format!(
                "the catalog declares that {product} needs the {other} {surface} by design: \
                 {because}"
            )),
            Standing::Bound {
                surface,
                because: None,
            } => Some(format!(
                "the catalog declares that {product} needs the {other} {surface} with no \
                 alternative and no bound_because saying why"
            )),
            Standing::Unproven { alternative } => Some(format!(
                "the declared alternative ({alternative}) names no alternative_source, so no \
                 code shows {product} running without {other}"
            )),
            Standing::SourceMissing { alternative_source } => Some(format!(
                "alternative_source {alternative_source} is not a file in the workspace"
            )),
        }
    }
}

/// The alternative of one declared need and the code that implements it, or
/// the standing that says why there is none.
fn proof(workspace: &Path, need: Dependency) -> Result<(String, String), Standing> {
    let Some(alternative) = need.alternative else {
        return Err(Standing::Bound {
            surface: need.surface,
            because: need.bound_because,
        });
    };
    let Some(alternative_source) = need.alternative_source else {
        return Err(Standing::Unproven { alternative });
    };
    let file = match alternative_source.split_once(':') {
        Some((file, _)) => file,
        None => alternative_source.as_str(),
    };
    if !workspace.join(file).is_file() {
        return Err(Standing::SourceMissing { alternative_source });
    }
    Ok((alternative, alternative_source))
}

/// The standing of `other` among `declared`, the catalog dependencies of the
/// product whose code reaches it. Every declared need of it must have an
/// alternative whose code exists; the first that does not decides.
pub fn judge(workspace: &Path, declared: &[Value], other: &str) -> Result<Standing> {
    let mut proven = Vec::new();
    for item in declared {
        let need = Dependency::deserialize(item).context("invalid catalog dependency")?;
        if need.product != other {
            continue;
        }
        match proof(workspace, need) {
            Ok(alternative) => proven.push(alternative),
            Err(standing) => return Ok(standing),
        }
    }
    let Some((alternative, alternative_source)) = proven.into_iter().next() else {
        return Ok(Standing::Undeclared);
    };
    Ok(Standing::Independent {
        alternative,
        alternative_source,
    })
}
