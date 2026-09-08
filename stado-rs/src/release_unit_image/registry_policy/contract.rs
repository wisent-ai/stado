//! The document-boundary refusal: what a `release_unit_image_revisit` block
//! must say before any host acts on it.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{policy, REVISIT_POLICY_KEY};

/// Every product a shipped declaration or an adopted service says owns
/// `label` on `target_name`.
///
/// These are positive witnesses, not prerequisites. An explicit revisit
/// policy remains sufficient when neither catalogue carries ownership, but it
/// may not contradict either catalogue. All witnesses are retained so a
/// contradictory registry cannot be hidden by whichever source happened to be
/// searched first.
fn declared_owners(
    document: &Value,
    target_name: &str,
    label: &str,
) -> Result<std::collections::BTreeSet<String>, String> {
    let mut owners = std::collections::BTreeSet::new();
    let products = crate::deploy::products::declared()
        .map_err(|error| format!("cannot read the shipped product declarations: {error}"))?;
    for product in products {
        if product
            .units
            .iter()
            .any(|unit| unit.label_for(target_name) == label)
        {
            owners.insert(product.name.clone());
        }
    }
    let services = document
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|target| target.get("name").and_then(Value::as_str) == Some(target_name))
        .filter_map(|target| target.get("services").and_then(Value::as_array))
        .flatten();
    for service in services {
        // Match `service::declared_services`: a declared-only record is a
        // pre-adoption placeholder, not a managed service and therefore not a
        // positive runtime ownership witness even when it carries onboarding.
        if service.get("declared_only").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        // Exact unit identity only. `ManagedService::matches` also accepts the
        // logical service name, which is deliberately broader than ownership.
        let named = |key: &str| {
            service
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        };
        if named("label").or_else(|| named("unit")) != Some(label) {
            continue;
        }
        if let Some(owner) = service
            .get("onboarding")
            .and_then(|value| value.get("product_id"))
            .and_then(Value::as_str)
        {
            owners.insert(owner.to_string());
        }
    }
    Ok(owners)
}

/// Refuse a `release_unit_image_revisit` block that cannot mean what it says.
///
/// Wired into [`crate::targets::validate_registry_body`] beside the other
/// extension validators, so an operator learns at the write and a build that
/// disagrees with the document reports it through the existing
/// `build-refuses-registry` finding rather than acting on half of it.
pub(crate) fn validate_registry_contract(document: &Value) -> Result<(), String> {
    let Some(policy) = policy(document)? else {
        return Ok(());
    };
    let location = format!("registry.{REVISIT_POLICY_KEY}");
    if policy.schema_version != 1 {
        return Err(format!("{location}.schema_version must be 1"));
    }
    let platforms: BTreeMap<&str, &str> = document
        .get("targets")
        .and_then(Value::as_array)
        .map(|targets| {
            targets
                .iter()
                .filter_map(|target| {
                    Some((
                        target.get("name")?.as_str()?,
                        target
                            .get("release_platform")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    for (target_name, target_policy) in &policy.targets {
        let at = format!("{location}.targets.{target_name}");
        let Some(platform) = platforms.get(target_name.as_str()) else {
            return Err(format!("{at}: unknown target"));
        };
        // launchd-only. The restart runs `launchctl kickstart -k`, which
        // exists only on Darwin, so every authorised label on a Linux target
        // would fail and record `RestartRefused` against the identity pair.
        // That record bars the pair, so it is not a hot loop — but it is not a
        // resting state either: each replacement of the declared file changes
        // the identity, expires the row, and buys one more `launchctl` call
        // that cannot succeed for the same reason as the last. The host spends
        // one futile restart per release indefinitely and records each as a
        // repair considered. Matched on `darwin-` and not `darwin` because
        // `release_platform` is an `<os>-<arch>` coordinate.
        if !platform.starts_with("darwin-") {
            return Err(format!(
                "{at}: release_platform is {platform:?}, and unit-image revisit restarts through \
                 launchctl, which only Darwin has"
            ));
        }
        if !crate::release_control::safe_absolute(&target_policy.state_dir) {
            return Err(format!(
                "{at}.state_dir must be an absolute path with no '..' component"
            ));
        }
        // One `(target, label)` has one owning product or no owner: two
        // claimants means no single product authorised the restart. Scoped to
        // the target, because two hosts may legitimately run units of the same
        // name — that is what a launchd label is.
        let mut owners: BTreeMap<&str, &str> = BTreeMap::new();
        for (product, units) in &target_policy.products {
            let at = format!("{at}.products.{product}");
            if !crate::targets::is_product_identifier(product) {
                return Err(format!("{at}: is not a canonical product name"));
            }
            if units.is_empty() {
                return Err(format!(
                    "{at}: declares no units; omit the product rather than authorising an empty \
                     list, so that off is spelled one way"
                ));
            }
            for unit in units {
                // The same shape every other canonical name in this document
                // is held to, plus the dot every launchd label carries.
                if !crate::release_control::identifier(unit) || !unit.contains('.') {
                    return Err(format!("{at}: {unit} is not a launchd label"));
                }
                if let Some(owner) = owners.insert(unit.as_str(), product.as_str()) {
                    return Err(format!(
                        "{at}: {unit} on {target_name} is already authorised by {owner}; a unit \
                         on one host has one authorising product"
                    ));
                }
                // Where ownership IS written down, the policy may not
                // contradict it. Silence is permission, but every positive
                // witness must agree; retaining all witnesses also exposes a
                // shipped/onboarding contradiction instead of letting search
                // order choose an owner.
                for owner in declared_owners(document, target_name, unit)? {
                    if owner != *product {
                        return Err(format!(
                            "{at}: {unit} is declared as owned by {owner}, so {product} cannot \
                             authorise restarting it"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
