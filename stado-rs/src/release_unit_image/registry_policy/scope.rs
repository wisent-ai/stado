//! What the document asked for on one target, and the host-wide contract that
//! resolves from it.

use std::collections::BTreeMap;

use super::RevisitPolicy;

/// Every launchd label the policy authorises on ONE target, and the product
/// that claimed it first.
///
/// Answers a different question from [`host_scope`]: what did the document ASK
/// FOR here. `registry doctor` needs that even when the contract does not
/// resolve, so it can say on the row that the agent will not act and why.
pub(in crate::release_unit_image) fn declared_units(
    policy: &RevisitPolicy,
    target_name: &str,
) -> BTreeMap<String, String> {
    let mut declared = BTreeMap::new();
    let Some(target) = policy.targets.get(target_name) else {
        return declared;
    };
    for (product, units) in &target.products {
        for unit in units {
            declared
                .entry(unit.clone())
                .or_insert_with(|| product.clone());
        }
    }
    declared
}

/// The host-wide revisit contract: where the ledger lives, and which product
/// authorises each launchd label on this target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisitScope {
    pub state_dir: String,
    /// launchd label to the product that authorised it, for this target only.
    pub units: BTreeMap<String, String>,
}

impl RevisitScope {
    /// The labels this process may act on, once `--product` is applied.
    ///
    /// A filtered invocation may act ONLY on that exact policy product; an
    /// unfiltered one may act on any of them. The filter narrows what is
    /// ACTED ON and never what the contract is computed from: the ledger path
    /// and the ownership map are host-wide, so a `--product brama` agent and a
    /// `--product skarbiec` agent share one ledger and one lock.
    pub(in crate::release_unit_image) fn owned(
        &self,
        product_filter: Option<&str>,
    ) -> BTreeMap<String, String> {
        self.units
            .iter()
            .filter(|(_, product)| product_filter.is_none_or(|want| want == product.as_str()))
            .map(|(unit, product)| (unit.clone(), product.clone()))
            .collect()
    }
}

/// The contract for one target, computed from EVERY product in the policy
/// regardless of `--product`, or `None` when nothing on this target is
/// authorised.
///
/// Every refusal here is one [`super::contract::validate_registry_contract`]
/// already made at the document boundary. It is held again at the point of use
/// because a document written by a looser build can still arrive, and acting
/// on half of an unresolvable policy is worse than reporting it.
pub(crate) fn host_scope(
    policy: &RevisitPolicy,
    target_name: &str,
) -> Result<Option<RevisitScope>, String> {
    let Some(target_policy) = policy.targets.get(target_name) else {
        return Ok(None);
    };
    if target_policy.products.is_empty() {
        return Ok(None);
    }
    let mut units: BTreeMap<String, String> = BTreeMap::new();
    for (product, labels) in &target_policy.products {
        for unit in labels {
            if let Some(owner) = units.insert(unit.clone(), product.clone()) {
                return Err(format!(
                    "({target_name}, {unit}) is authorised by both {owner} and {product}; a unit \
                     on one host has one authorising product"
                ));
            }
        }
    }
    Ok(Some(RevisitScope {
        state_dir: target_policy.state_dir.clone(),
        units,
    }))
}
