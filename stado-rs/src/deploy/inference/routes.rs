use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::inference::schema::Registry;
use crate::targets::ComputeTarget;

fn report(target: &ComputeTarget, output: &crate::deploy::CommandOutput, ok: &str) -> Value {
    let mut body = host_channel::base_report(target);
    host_channel::finish_report(&mut body, output, ok, "inference route operation failed");
    Value::Object(body)
}

pub fn transaction(registry: &Registry) -> Result<String, DeployError> {
    let body = serde_json::to_vec(registry).map_err(|error| DeployError(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(body)))
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
        return Err(
            DeployError("invalid inference route transaction".to_string())
                .stating(crate::primitives::failure::FailureCode::Refused),
        );
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
        return Err(
            DeployError("invalid inference route transaction".to_string())
                .stating(crate::primitives::failure::FailureCode::Refused),
        );
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
        return Err(
            DeployError("invalid inference route transaction".to_string())
                .stating(crate::primitives::failure::FailureCode::Refused),
        );
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
/// on either side saying so: the canonical registry can lose a route while a
/// host keeps serving it, invisible to every command. `absent` is an answer: a host
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
        return Err(DeployError::unreachable(format!(
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
            serde_json::from_str(&body).map(Some).map_err(|error| {
                DeployError::unreachable(format!(
                    "{}: the gateway route table is not JSON: {error}",
                    target.name
                ))
            })
        }
        _ => Err(DeployError::unreachable(format!(
            "{}: the gateway route table read answered nothing this command understands",
            target.name
        ))),
    }
}

/// Whether each of `aliases` is answered by the gateway on TARGET right now:
/// one real completion per alias through Brama's own `brama probe`, run on
/// the gateway host in its service environment and presenting the client
/// bearer the item playing `bearer_role` (`<role>#<field>`) holds. A table
/// that agrees with its declaration can still route to a provider that
/// refuses every request (an exhausted account), and only a request shows
/// that. Each alias spends one short provider request, so the caller asks
/// for it explicitly. The answer is Brama's own JSON report: per alias its
/// HTTP status, and the model and words, or the refusal or transport error.
pub async fn answers(
    target: &ComputeTarget,
    aliases: &[String],
    bearer_role: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let named = aliases
        .iter()
        .map(|alias| format!("--alias {}", shlex_quote(alias)))
        .collect::<Vec<_>>()
        .join(" ");
    let role = shlex_quote(bearer_role);
    let script = format!(
        r#"set -euo pipefail
brama="$HOME/.stado/bin/brama"
if [ ! -x "$brama" ]; then
  printf 'STATUS\tbrama_absent\n'
  exit 0
fi
printf 'STATUS\tprobe_ran\n'
"$brama" probe --json --allow-provider-cost --bearer-role {role} {named} || true
"#
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError::unreachable(format!(
            "{}: the gateway probe could not run: {}",
            target.name,
            host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    let mut lines = output.stdout.lines();
    match lines.next().map(str::trim) {
        Some("STATUS\tbrama_absent") => Err(DeployError(format!(
            "{}: no Brama is installed at ~/.stado/bin/brama, so no alias can be probed there",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)),
        Some("STATUS\tprobe_ran") => {
            let body = lines.collect::<Vec<_>>().join("\n");
            serde_json::from_str(&body).map_err(|error| {
                DeployError::unreachable(format!(
                    "{}: brama probe answered no JSON report ({error}); its error: {}",
                    target.name,
                    host_channel::last_error_line(&output, "none")
                ))
            })
        }
        _ => Err(DeployError::unreachable(format!(
            "{}: the gateway probe answered nothing this command understands",
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
