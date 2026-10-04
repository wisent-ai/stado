//! Which release control plane, which product, which host: the three lookups
//! both quarantine commands and every reader outside this module start from.

use crate::cli::registry;
use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::primitives::failure::FailureCode;
use crate::release_control::{ProductReleasePolicy, ReleaseControl, ReleaseTargetPolicy};
use crate::targets::ComputeTarget;

/// The registry's release control plane, or the reason there is none.
pub(crate) async fn canonical_control() -> Result<ReleaseControl, CmdError> {
    let document = registry::fetch_document().await?;
    crate::release_control::control(&document)
        .map_err(|error| CmdError::click(error).stating(FailureCode::Config))?
        .ok_or_else(|| {
            CmdError::click("registry.release_control is not configured")
                .stating(FailureCode::Config)
        })
}

/// The product policy and one of its targets.
///
/// `--target` may be omitted only while the product rolls out to exactly one
/// host. Guessing among several would put a write on whichever host sorted
/// first, which is the kind of help nobody asked for.
pub(crate) fn resolve_target<'a>(
    control: &'a ReleaseControl,
    product: &str,
    target: Option<&str>,
) -> Result<(String, &'a ProductReleasePolicy, &'a ReleaseTargetPolicy), CmdError> {
    let policy = control
        .products
        .get(product)
        .ok_or_else(|| crate::cli::release_cmd::unknown_release_product(control, product))?;
    let name = match target {
        Some(named) => named.to_string(),
        None => {
            let mut names = policy.targets.keys();
            match (names.next(), names.next()) {
                (Some(only), None) => only.clone(),
                (Some(_), Some(_)) => {
                    let declared: Vec<&str> = policy.targets.keys().map(String::as_str).collect();
                    return Err(CmdError::usage(format!(
                        "{product} rolls out to {}; name one with --target",
                        declared.join(", ")
                    )));
                }
                _ => {
                    return Err(
                        CmdError::click(format!("{product} declares no release target"))
                            .stating(FailureCode::Config),
                    )
                }
            }
        }
    };
    let target_policy = policy.targets.get(&name).ok_or_else(|| {
        CmdError::click(format!("{product} does not roll out to {name:?}"))
            .stating(FailureCode::NotFound)
    })?;
    Ok((name, policy, target_policy))
}

/// The registry-authorized host behind a release target name.
pub(crate) async fn compute_target(name: &str) -> Result<ComputeTarget, CmdError> {
    host_channel::canonical_target(name)
        .await
        .map_err(|error| CmdError::click(error.to_string()))
}
