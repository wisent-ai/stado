//! Issuing one capability on the host that will redeem it, and storing the
//! one Apple challenge whose digits never leave stdin.

use serde_json::Value;

use super::{run_json, RemoteBroker};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// What one capability asks for, in Skarbiec's own vocabulary.
pub struct Issuance<'a> {
    pub agent: &'a str,
    pub purpose: &'a str,
    pub resource: &'a str,
    /// The consumer the reference is for — `weles` for a browser fill.
    pub capability_target: &'a str,
    pub ttl_seconds: &'a str,
    pub max_uses: &'a str,
    /// Skarbiec binds a capability to an authorization id when one is given,
    /// and redemption then requires the redeemer to present the same one.
    /// Whether to bind is the CONSUMER's contract, not a preference: Weles's
    /// Apple sign-in builds its expectation with its guard id, while
    /// `wsFillCredential` builds `{ purpose, resource }` and nothing else, so
    /// a browser fill must be issued and referenced WITHOUT one or every
    /// redemption throws `capability operation mismatch`.
    pub authorization_id: Option<&'a str>,
}

/// Issue one capability on the target and return its id.
///
/// Skarbiec's own bounds: `--ttl` is whole seconds up to 3600, `--max-uses`
/// is 1..=16, and a resource with no route is refused at issue time rather
/// than at redemption — which is the whole reason issuing happens here, in
/// front of a flow that would otherwise spend its one fill discovering it.
pub async fn issue(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    issuance: &Issuance<'_>,
    runner: &Runner,
) -> Result<String, DeployError> {
    let mut arguments = vec![
        "grant",
        "capability",
        "--agent",
        issuance.agent,
        "--purpose",
        issuance.purpose,
        "--resource",
        issuance.resource,
        "--target",
        issuance.capability_target,
        "--ttl",
        issuance.ttl_seconds,
        "--max-uses",
        issuance.max_uses,
    ];
    if let Some(authorization_id) = issuance.authorization_id {
        arguments.push("--authorization-id");
        arguments.push(authorization_id);
    }
    let issued = run_json(target, broker, &arguments, runner).await?;
    issued
        .get("capability_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            DeployError(format!(
                "{}: skarbiec issued no capability id for {}, so nothing could be redeemed",
                target.name, issuance.resource
            ))
        })
}

/// Store one captured Apple code in the Weles broker that will redeem it.
///
/// The six digits travel on stdin only. They are never an argument, a registry
/// value, a diagnostic, or part of this function's receipt.
pub async fn apple_challenge_put(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    resource: &str,
    code: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DeployError(
            "refusing to store an invalid Apple challenge".to_string(),
        ));
    }
    let command = broker.command(&["apple-challenge-put", resource]);
    let output =
        host_channel::run_program_with_stdin(target, &["/bin/sh", "-c", &command], code, runner)
            .await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: `skarbiec apple-challenge-put` failed against {}: {}",
            target.name,
            broker.vault,
            host_channel::last_error_line(&output, "the host gave no reason")
        )));
    }
    let receipt: Value = serde_json::from_str(output.stdout.trim()).map_err(|error| {
        DeployError(format!(
            "{}: `skarbiec apple-challenge-put` did not answer with JSON: {error}",
            target.name
        ))
    })?;
    if receipt.get("status").and_then(Value::as_str) != Some("stored") {
        return Err(DeployError(format!(
            "{}: Skarbiec did not confirm that the Apple challenge was stored",
            target.name
        )));
    }
    Ok(receipt)
}
