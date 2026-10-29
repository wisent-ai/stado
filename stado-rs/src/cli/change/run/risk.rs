//! The deterministic first half of the risk classifier: the product's
//! `submit` declaration in the catalog (`stado_product::catalog::SubmitPolicy`)
//! confronted with what the rebased change touches and with the gates that
//! will check it. A change is high risk for every reason that holds, each
//! named; a low-risk change names why it is low. The reviewer is the second
//! half (`review`), which runs before landing for high and after for low.

use stado_product::catalog::{Criticality, SubmitPolicy};

use crate::cli::change::record::Risk;
use crate::cli::CmdError;

pub const HIGH: &str = "high";
pub const LOW: &str = "low";

/// The `submit` declaration of `product`'s catalog record, read from the
/// catalog this machine's products are installed from.
pub fn policy(product: &str) -> Result<SubmitPolicy, CmdError> {
    let runtime = stado_product::common::Runtime::new(None).map_err(|error| {
        CmdError::declaration(format!("the product catalog has no place: {error:#}"))
    })?;
    let catalog = stado_product::catalog::current(&runtime).map_err(|error| {
        CmdError::declaration(format!(
            "the product catalog {} cannot be read: {error:#}",
            runtime.catalog.display()
        ))
    })?;
    let record = stado_product::catalog::product(&catalog, product).map_err(|error| {
        CmdError::declaration(format!(
            "{error:#} in {}; the submit queue classifies a change by its product's catalog \
             record",
            runtime.catalog.display()
        ))
    })?;
    stado_product::catalog::submit_policy(product, record)
        .map_err(|error| CmdError::declaration(format!("{error:#}")))?
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "{product}'s record in {} declares no submit policy, so the queue cannot say \
                 whether a change of it is reviewed before or after it lands; declare \
                 `submit: {{criticality: critical}}` or `submit: {{criticality: standard, \
                 low_risk_max_changed_lines: N}}`, with any `review_paths`, on it",
                runtime.catalog.display()
            ))
        })
}

/// The classification of a change of `product` that touches `paths` and
/// `lines` lines, checked by `compiling` lint and test gates.
pub fn classify(
    product: &str,
    policy: &SubmitPolicy,
    paths: &[String],
    lines: usize,
    compiling: &[String],
) -> Risk {
    let mut high = Vec::new();
    if policy.criticality == Criticality::Critical {
        high.push(format!(
            "the catalog declares {product} {}",
            Criticality::Critical.word()
        ));
    }
    for path in paths {
        if let Some(declared) = policy.review_path(path) {
            high.push(format!(
                "it changes {path}, under the review path {declared} {product} declares"
            ));
        }
    }
    // Validation guarantees a standard product declares the number; a
    // critical one is high risk above whatever its size.
    let max = policy.low_risk_max_changed_lines.unwrap_or(usize::MAX);
    if lines > max {
        high.push(format!(
            "it changes {lines} lines, more than the {max} a low-risk change of {product} may"
        ));
    }
    if compiling.is_empty() {
        high.push(format!(
            "no lint or test gate of {product} compiles or runs it; only its format gates read it"
        ));
    }
    let (level, reasons) = if high.is_empty() {
        (
            LOW,
            vec![format!(
                "{product} is {}; it changes {lines} of at most {max} lines, under no review \
                 path; gates {} check it",
                policy.criticality.word(),
                compiling.join(", ")
            )],
        )
    } else {
        (HIGH, high)
    };
    Risk {
        level: level.to_string(),
        reasons,
        changed_files: paths.len(),
        changed_lines: lines,
    }
}
