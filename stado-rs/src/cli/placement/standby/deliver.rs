//! Getting one service's program onto the standby host: a managed program is
//! delivered through the same manifest-verified path `stado release
//! host-state --apply` uses; a release-controlled tree is declared as a
//! rollout target for the host, and the host's own agent stages it.

use serde::Serialize;
use serde_json::{json, Value};

use super::template::{ProgramKind, ServicePlan};
use crate::cli::CmdError;
use crate::deploy::host_release;
use crate::targets::ComputeTarget;

/// The words a delivery ends with.
pub mod words {
    /// The managed program is on the host at the declared version.
    pub const DELIVERED: &str = "delivered";
    /// The host was just written into the product's release control as a
    /// rollout target; its agent stages the release on its next drift check.
    pub const ROLLOUT_DECLARED: &str = "rollout_declared";
    /// The host is a rollout target already and the tree is not on it yet.
    pub const ROLLOUT_PENDING: &str = "rollout_pending";
    /// The host is a rollout target and runs the tree.
    pub const ROLLED_OUT: &str = "rolled_out";
}

#[derive(Debug, Clone, Serialize)]
pub struct Delivery {
    pub service: String,
    pub kind: String,
    pub product: String,
    pub version: Option<String>,
    pub outcome: String,
    pub detail: String,
}

pub(super) async fn deliver(
    document: &Value,
    placed: &ComputeTarget,
    target: &ComputeTarget,
    service: &ServicePlan,
) -> Result<Delivery, CmdError> {
    match &service.kind {
        ProgramKind::Managed { product } => deliver_program(placed, target, service, product).await,
        ProgramKind::Tree { product } => {
            declare_rollout(document, placed, target, service, product).await
        }
    }
}

async fn deliver_program(
    placed: &ComputeTarget,
    target: &ComputeTarget,
    service: &ServicePlan,
    product: &str,
) -> Result<Delivery, CmdError> {
    let declaration = crate::deploy::products::product(product)
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !declaration
        .platforms
        .iter()
        .any(|platform| platform == &target.release_platform)
    {
        return Err(CmdError::click(format!(
            "{product} is published for {} only; {} declares release_platform {}, so {} cannot \
             stand by there until a {product} release for that platform exists",
            declaration.platforms.join(", "),
            target.name,
            target.release_platform,
            service.logical
        )));
    }
    let version = target
        .managed_versions
        .get(product)
        .or_else(|| placed.managed_versions.get(product))
        .cloned()
        .ok_or_else(|| {
            CmdError::click(format!(
                "neither {} nor the placed host {} declares a {product} version under \
                 targets[].managed_versions; declare one with `stado release declare-version \
                 --host {} --binary {product} --version X.Y.Z` so the standby is delivered the \
                 version the fleet runs",
                target.name, placed.name, target.name
            ))
        })?;
    let runner = crate::deploy::production_runner();
    let report = host_release::release_host(&target.name, product, &version, false, false, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let status = report
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(
        status,
        host_release::RELEASED_STATUS | host_release::ALREADY_ACTIVE_STATUS
    ) {
        let cause = report
            .get("error")
            .and_then(Value::as_str)
            .filter(|error| !error.is_empty())
            .unwrap_or(status);
        return Err(CmdError::click(format!(
            "{}: delivering {product} {version} for {} did not complete: {cause}",
            target.name, service.logical
        )));
    }
    Ok(Delivery {
        service: service.logical.clone(),
        kind: "managed_program".to_string(),
        product: product.to_string(),
        version: Some(version),
        outcome: words::DELIVERED.to_string(),
        detail: status.to_string(),
    })
}

/// The directories a release-controlled product keeps under one host's home.
const RELEASE_STATE_LEAF: &str = ".stado/release-state";
const RELEASE_RUNTIME_LEAF: &str = ".stado/run";
const RELEASE_LOGS_LEAF: &str = ".stado/logs";

async fn declare_rollout(
    document: &Value,
    placed: &ComputeTarget,
    target: &ComputeTarget,
    service: &ServicePlan,
    product: &str,
) -> Result<Delivery, CmdError> {
    let policy = document
        .get("release_control")
        .and_then(|control| control.get("products"))
        .and_then(|products| products.get(product))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} runs {product} as a release-controlled tree, but registry.release_control \
                 declares no product named {product:?}; nothing rolls it out to {}",
                service.logical, target.name
            ))
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
        return Err(CmdError::click(format!(
            "{product} {} is published with no {} artifact; {} cannot stand by on {} until a \
             {product} release builds for {}: `stado release submit --source <checkout> \
             --commit <sha> --version <next>` queues one for every platform the product declares",
            desired_version.as_deref().unwrap_or("(undesired)"),
            target.release_platform,
            service.logical,
            target.name,
            target.release_platform
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
        return Ok(Delivery {
            service: service.logical.clone(),
            kind: "release_tree".to_string(),
            product: product.to_string(),
            version: desired_version,
            outcome: if running {
                words::ROLLED_OUT.to_string()
            } else {
                words::ROLLOUT_PENDING.to_string()
            },
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
            CmdError::click(format!(
                "release control for {product} declares no target for the placed host {}; there \
                 is no serving declaration to derive {}'s from",
                placed.name, target.name
            ))
        })?;
    let home = crate::deploy::service_catalog::home_for(target);
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
        })?;
    let mut entry = json!({
        "platform": target.release_platform,
        "run_as_user": run_as_user,
        "home": home,
        "state_dir": format!("{home}/{RELEASE_STATE_LEAF}"),
        "runtime_root": format!("{home}/{RELEASE_RUNTIME_LEAF}"),
        "logs_root": format!("{home}/{RELEASE_LOGS_LEAF}"),
    });
    for key in ["stable_bind", "candidate_ports", "readiness_path"] {
        if let Some(value) = source.get(key) {
            entry[key] = value.clone();
        }
    }
    let product_name = product.to_string();
    let target_name = target.name.clone();
    let generation = crate::cli::registry::commit_document(move |current| {
        let mut next = current.clone();
        let control = next
            .get_mut("release_control")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| CmdError::click("registry.release_control is not an object"))?;
        let generation = control
            .get("generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                CmdError::click("registry.release_control.generation is not an integer")
            })?;
        let targets = control
            .get_mut("products")
            .and_then(|products| products.get_mut(&product_name))
            .and_then(|policy| policy.get_mut("targets"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "release control for {product_name} lost its targets object"
                ))
            })?;
        if targets.contains_key(&target_name) {
            return Ok(current.clone());
        }
        targets.insert(target_name.clone(), entry.clone());
        control.insert(
            "generation".to_string(),
            Value::from(generation.saturating_add(1)),
        );
        Ok(next)
    })
    .await?;
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
