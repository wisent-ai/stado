//! A release-controlled tree reaches a standby host by being declared a
//! rollout target in the release control that rolls it out; the host's own
//! agent then stages it.

mod roots;

use serde_json::{json, Value};

use self::roots::{home_relative, installs_at_tree};
use super::super::template::ServicePlan;
use super::tree::install_tree;
use super::{words, Delivery};
use crate::cli::CmdError;
use crate::release_control::HOME_PLACEHOLDER;
use crate::targets::ComputeTarget;

/// The directories a release-controlled product keeps under one host's home.
const RELEASE_STATE_LEAF: &str = ".stado/release-state";
const RELEASE_RUNTIME_LEAF: &str = ".stado/run";
const RELEASE_LOGS_LEAF: &str = ".stado/logs";

pub(super) async fn declare_rollout(
    document: &Value,
    placed: &ComputeTarget,
    target: &ComputeTarget,
    service: &ServicePlan,
    product: &str,
) -> Result<Delivery, CmdError> {
    // A tree is rolled out by the release-control product of its name, or by
    // the one whose `service` names it (weles-worker rolls out weles-admission).
    let tree = product;
    let products = document
        .get("release_control")
        .and_then(|control| control.get("products"));
    let serving = |(_, policy): &(&String, &Value)| {
        policy.get("service").and_then(Value::as_str) == Some(product)
    };
    let (product, policy) = products
        .and_then(Value::as_object)
        .and_then(|products| {
            products
                .get_key_value(product)
                .or_else(|| products.iter().find(serving))
        })
        .map(|(name, policy)| (name.as_str(), policy))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} runs {product} as a release-controlled tree, but registry.release_control \
                 declares no product named {product:?} or serving it; nothing rolls it out to {}",
                service.logical, target.name
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let desired_version = policy
        .get("desired")
        .and_then(|desired| desired.get("version"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let has_artifact = policy
        .get("desired")
        .and_then(|desired| desired.get("artifacts"))
        .and_then(|artifacts| artifacts.get(&target.release_platform))
        .is_some();
    if !has_artifact {
        let desired = match desired_version.as_deref() {
            Some(v) => v,
            None => "(undesired)",
        };
        return Err(CmdError::refused(format!(
            "{product} {desired} is published with no {} artifact; {} cannot stand by on {} until a \
             {product} release builds for {}: `stado release submit --source <checkout> \
             --commit <sha> --version <next>` queues one for every platform the product declares",
            target.release_platform, service.logical, target.name, target.release_platform
        )));
    }
    let declared = policy
        .get("targets")
        .and_then(|targets| targets.get(&target.name))
        .is_some();
    if declared {
        let running = crate::deploy::service::declared_services(target)
            .iter()
            .any(|managed| managed.matches(&service.catalog_name));
        // A tree its release control installs elsewhere, or under another
        // host's home, is installed here (the archive installer leaves an
        // active version be).
        let root = policy.get("install_root").and_then(Value::as_str);
        let home =
            crate::deploy::host_channel::remote_home(target, &crate::deploy::production_runner())
                .await
                .map_err(CmdError::from)?;
        let foreign =
            |root: &str| !root.contains(HOME_PLACEHOLDER) && !root.starts_with(&format!("{home}/"));
        // A root under the placed host's home is rewritten to the `{home}`
        // form first, so this target's agent stages later releases at home.
        let placed_home = policy
            .get("targets")
            .and_then(|targets| targets.get(&placed.name))
            .and_then(|placed| placed.get("home"))
            .and_then(Value::as_str);
        if let Some(portable) = root
            .filter(|root| foreign(root))
            .zip(placed_home)
            .and_then(|(root, home)| home_relative(root, home))
        {
            super::tree::rewrite_install_root(product, &portable).await?;
        }
        if !running && root.is_some_and(|root| !installs_at_tree(root, tree) || foreign(root)) {
            let installed = install_tree(product, tree, target, service).await?;
            return Ok(Delivery {
                service: service.logical.clone(),
                kind: "release_tree".to_string(),
                product: product.to_string(),
                version: desired_version,
                outcome: words::DELIVERED.to_string(),
                detail: format!("{} is a {product} rollout target; {installed}", target.name),
            });
        }
        return Ok(Delivery {
            service: service.logical.clone(),
            kind: "release_tree".to_string(),
            product: product.to_string(),
            version: desired_version,
            outcome: if running {
                words::ROLLED_OUT
            } else {
                words::ROLLOUT_PENDING
            }
            .to_string(),
            detail: format!(
                "{} is a {product} rollout target; its agent stages the desired release",
                target.name
            ),
        });
    }
    let source = policy
        .get("targets")
        .and_then(|targets| targets.get(&placed.name))
        .cloned()
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "release control for {product} declares no target for the placed host {}; there \
                 is no serving declaration to derive {}'s from",
                placed.name, target.name
            ))
        })?;
    let placed_home = source.get("home").and_then(Value::as_str).ok_or_else(|| {
        CmdError::declaration(format!(
            "release control for {product} names no home for the placed host {}",
            placed.name
        ))
    })?;
    let install_root = policy
        .get("install_root")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "release control for {product} names no install_root"
            ))
        })?;
    let portable_root = home_relative(install_root, placed_home);
    // The home the host's own shell states, not one derived from the SSH user.
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(target, &runner)
        .await
        .map_err(CmdError::from)?;
    if !install_root.contains(HOME_PLACEHOLDER) && portable_root.is_none() {
        return Err(CmdError::refused(format!(
            "release control for {product} installs every host at {install_root}, which is \
             neither under {HOME_PLACEHOLDER} nor under {}'s home {placed_home}; {} would install \
             into a path that is not its own",
            placed.name, target.name
        )));
    }
    // The unit standby stages runs the catalog program under
    // `.stado/services/<tree>/current/`; a release control that installs
    // elsewhere does not write it, so the tree is installed there directly.
    let rooted = match &portable_root {
        Some(root) => root.clone(),
        None => install_root.to_string(),
    };
    let run_as_user = target
        .ssh
        .as_deref()
        .and_then(|ssh| ssh.split_once('@'))
        .map(|(user, _)| user.to_string())
        .or_else(|| {
            source
                .get("run_as_user")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} declares no ssh account to run {product} as",
                target.name
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let mut entry = json!({
        "platform": target.release_platform,
        "run_as_user": run_as_user,
        "home": home,
        "state_dir": format!("{home}/{RELEASE_STATE_LEAF}"),
        "runtime_root": format!("{home}/{RELEASE_RUNTIME_LEAF}"),
        "logs_root": format!("{home}/{RELEASE_LOGS_LEAF}"),
    });
    // How the product serves is the same on every host; where it was
    // installed before release control (its legacy unit) is the placed host's.
    let mut keep = |key: &str| {
        if let Some(value) = source.get(key) {
            entry[key] = value.clone();
        }
    };
    keep("stable_bind");
    keep("candidate_ports");
    keep("readiness_path");
    let product_name = product.to_string();
    let target_name = target.name.clone();
    let generation = crate::cli::registry::commit_document(move |current| {
        let mut next = current.clone();
        let control = next
            .get_mut("release_control")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click("registry.release_control is not an object")
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        let generation = control
            .get("generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                CmdError::click("registry.release_control.generation is not an integer")
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        let policy = control
            .get_mut("products")
            .and_then(|products| products.get_mut(&product_name))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(format!("release control lost its product {product_name}"))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        let targets = policy
            .get_mut("targets")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "release control for {product_name} lost its targets object"
                ))
                .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        if targets.contains_key(&target_name) {
            return Ok(current.clone());
        }
        targets.insert(target_name.clone(), entry.clone());
        if let Some(root) = &portable_root {
            policy.insert("install_root".to_string(), Value::from(root.clone()));
        }
        // The registry's generation counts commits; this commit is one more.
        let mut next_generation = generation;
        crate::primitives::tally::Tally::tally(&mut next_generation);
        control.insert("generation".to_string(), Value::from(next_generation));
        Ok(next)
    })
    .await?;
    if !installs_at_tree(&rooted, tree) {
        let installed = install_tree(product, tree, target, service).await?;
        return Ok(Delivery {
            service: service.logical.clone(),
            kind: "release_tree".to_string(),
            product: product.to_string(),
            version: desired_version,
            outcome: words::DELIVERED.to_string(),
            detail: format!("{} is now a {product} rollout target (registry generation {generation}); {installed}", target.name),
        });
    }
    Ok(Delivery {
        service: service.logical.clone(),
        kind: "release_tree".to_string(),
        product: product.to_string(),
        version: desired_version,
        outcome: words::ROLLOUT_DECLARED.to_string(),
        detail: format!(
            "{} is now a {product} rollout target (registry generation {generation}); its agent \
             stages the desired release on its next drift check",
            target.name
        ),
    })
}
