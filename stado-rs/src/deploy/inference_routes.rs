use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{host_channel, shlex_quote, DeployError, Runner};
use crate::inference::schema::Registry;
use crate::targets::ComputeTarget;

fn report(target: &ComputeTarget, output: &super::CommandOutput, ok: &str) -> Value {
    let mut body = host_channel::base_report(target);
    host_channel::finish_report(&mut body, output, ok, "inference route operation failed");
    Value::Object(body)
}

pub fn transaction(registry: &Registry) -> Result<String, DeployError> {
    let body = serde_json::to_vec(registry).map_err(|error| DeployError(error.to_string()))?;
    let digest = format!("{:x}", Sha256::digest(body));
    let one = usize::from(u8::from(true));
    let two = one.saturating_add(one);
    let width = Sha256::output_size() / two;
    Ok(digest.chars().take(width).collect())
}

fn valid_transaction(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub async fn stage(
    target: &ComputeTarget,
    registry: &Registry,
    transaction: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !valid_transaction(transaction) {
        return Err(DeployError(
            "invalid inference route transaction".to_string(),
        ));
    }
    let body = serde_json::to_vec(registry).map_err(|error| DeployError(error.to_string()))?;
    let encoded = shlex_quote(&STANDARD.encode(body));
    let transaction = shlex_quote(transaction);
    let script = format!(
        r#"set -euo pipefail
directory="$HOME/.stado/inference"
transaction={transaction}
mkdir -p "$directory"
chmod 700 "$directory"
printf '%s' {encoded} | base64 --decode > "$directory/routes.$transaction.json"
chmod 600 "$directory/routes.$transaction.json"
printf 'STATUS\troutes_staged\n'
"#
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "routes_staged"))
}

pub async fn commit(
    target: &ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !valid_transaction(transaction) {
        return Err(DeployError(
            "invalid inference route transaction".to_string(),
        ));
    }
    let transaction = shlex_quote(transaction);
    let script = format!(
        r#"set -euo pipefail
directory="$HOME/.stado/inference"
transaction={transaction}
test -f "$directory/routes.$transaction.json"
mv "$directory/routes.$transaction.json" "$directory/routes.json"
printf 'STATUS\troutes_committed\n'
"#
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "routes_committed"))
}

pub async fn discard(
    target: &ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !valid_transaction(transaction) {
        return Err(DeployError(
            "invalid inference route transaction".to_string(),
        ));
    }
    let transaction = shlex_quote(transaction);
    let script = format!(
        "set -euo pipefail\nrm -f \"$HOME/.stado/inference/routes.\"{transaction}\".json\"\nprintf 'STATUS\\troutes_discarded\\n'\n"
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "routes_discarded"))
}

/// The route table the gateway process on TARGET actually reads.
///
/// [`stage`] writes the whole serialized [`Registry`], so this file and the
/// canonical registry agree by construction — but only for a write that went
/// through this module. Any other writer of `registry.json` moves the
/// declaration and leaves the gateway serving the previous table, with nothing
/// on either side saying so. On 2026-09-06 the canonical registry lost the
/// `wisent-backend` route while `charless-mac-mini` kept serving it, and the
/// divergence was invisible to every command. `absent` is an answer: a host
/// that has never been staged has no file, and that is not a failure to read.
pub async fn live(target: &ComputeTarget, runner: &Runner) -> Result<Option<Value>, DeployError> {
    let script = r#"set -euo pipefail
file="$HOME/.stado/inference/routes.json"
if [ ! -f "$file" ]; then
  printf 'STATUS\troutes_absent\n'
  exit 0
fi
printf 'STATUS\troutes_present\n'
cat "$file"
"#;
    let output = host_channel::run_script(target, script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: the gateway route table could not be read: {}",
            target.name,
            host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    let mut lines = output.stdout.lines();
    match lines.next().map(str::trim) {
        Some("STATUS\troutes_absent") => Ok(None),
        Some("STATUS\troutes_present") => {
            let body = lines.collect::<Vec<_>>().join("\n");
            serde_json::from_str(&body)
                .map(Some)
                .map_err(|error| DeployError(format!("{}: {error}", target.name)))
        }
        _ => Err(DeployError(format!(
            "{}: the gateway route table read answered nothing this command understands",
            target.name
        ))),
    }
}

pub fn ready(value: &Value, state: &str) -> bool {
    value.get("status").and_then(Value::as_str) == Some(state)
}

pub fn summary(transaction: &str, stage: Value, commit: Value) -> Value {
    json!({"transaction": transaction, "stage": stage, "commit": commit})
}
