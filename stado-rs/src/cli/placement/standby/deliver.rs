//! Getting one service's program onto the standby host: a managed program is
//! delivered through the same manifest-verified path `stado release
//! version converge` uses; a release-controlled tree is declared as a
//! rollout target for the host, and the host's own agent stages it.

use serde::Serialize;
use serde_json::Value;

mod rollout;
mod tree;

use super::template::{ProgramKind, ServicePlan};
use crate::cli::CmdError;
use crate::deploy::host_release;
use crate::targets::ComputeTarget;
use rollout::declare_rollout;

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

/// Deliver `service` to `target`, as a wait named `what` on that host.
pub(super) async fn deliver(
    document: &Value,
    placed: &ComputeTarget,
    target: &ComputeTarget,
    service: &ServicePlan,
    what: String,
) -> Result<Delivery, CmdError> {
    let work = async {
        match &service.kind {
            ProgramKind::Managed { product } => {
                deliver_program(placed, target, service, product).await
            }
            ProgramKind::Tree { product } => {
                declare_rollout(document, placed, target, service, product).await
            }
        }
    };
    crate::wait::until(crate::wait::Kind::Host, what, &target.name, work).await
}

async fn deliver_program(
    placed: &ComputeTarget,
    target: &ComputeTarget,
    service: &ServicePlan,
    product: &str,
) -> Result<Delivery, CmdError> {
    let declaration = crate::deploy::products::product(product).map_err(CmdError::from)?;
    if !declaration
        .platforms
        .iter()
        .any(|platform| platform == &target.release_platform)
    {
        return Err(CmdError::refused(format!(
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
                 targets[].managed_versions; declare one with `stado release version declare \
                 --host {} --binary {product} --version X.Y.Z` so the standby is delivered the \
                 version the fleet runs",
                target.name, placed.name, target.name
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let runner = crate::deploy::production_runner();
    let report = host_release::release_host(&target.name, product, &version, false, false, &runner)
        .await
        .map_err(CmdError::from)?;
    // The release report says what the host did in `status`; one without
    // it is not a report this command can act on and is quoted whole.
    let Some(status) = report.get("status").and_then(Value::as_str) else {
        return Err(CmdError::click(format!(
            "{}: delivering {product} {version} for {} answered a report without a status: {report}",
            target.name, service.logical
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    };
    if !matches!(
        status,
        host_release::RELEASED_STATUS | host_release::ALREADY_ACTIVE_STATUS
    ) {
        // The host's error names the cause when it stated one; the status
        // word is the cause otherwise.
        let cause = match report.get("error").and_then(Value::as_str) {
            Some(error) if !error.is_empty() => error,
            Some(_) | None => status,
        };
        return Err(CmdError::click(format!(
            "{}: delivering {product} {version} for {} did not complete: {cause}",
            target.name, service.logical
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
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
