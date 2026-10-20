use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::machine::config::remote::{remote_config_output, RemoteConfigAction};
use crate::cli::host::machine::config::write_host_config;

/// The recovery program this command sends, assembled from its four
/// fragments in exactly this order: the prologue, the Skarbiec bootstrap
/// reconciliation, the route inspection, and the recovery itself. The shell
/// keeps the launchd, sudo and curl steps; every reader of JSON, plist and
/// process state it needs is the host's own `stado host local api`.
const RECOVERY_PROGRAM: &str = concat!(
    include_str!("../../../../../../deploy/recover_object_api/prologue.sh"),
    include_str!("../../../../../../deploy/recover_object_api/skarbiec.sh"),
    include_str!("../../../../../../deploy/recover_object_api/routes.sh"),
    include_str!("../../../../../../deploy/recover_object_api/recovery.sh"),
);

/// The program the recovery runs its readers with: the host's delivered Stado,
/// the `program` its prologue names.
const HOST_PROGRAM: &str = "$HOME/.stado/bin/stado";

/// Every `host local api` verb the recovery program runs, read from the
/// program itself: each line's words after `"$program" host local api `, up
/// to the first option or quoted argument. Read rather than listed, so a verb
/// added to the program is checked without a second list to keep.
fn host_verbs() -> std::collections::BTreeSet<String> {
    RECOVERY_PROGRAM
        .lines()
        .filter_map(|line| line.split_once("\"$program\" host local api "))
        .map(|(_, rest)| {
            rest.split_whitespace()
                .take_while(|word| !word.starts_with('-') && !word.starts_with('"'))
                .collect::<Vec<&str>>()
                .join(" ")
        })
        .filter(|verb| !verb.is_empty())
        .collect()
}

/// The shell that asks the host's Stado, before anything runs, for each verb
/// the program needs: a host Stado that predates one is refused naming the
/// verb and what that Stado said, instead of failing halfway through with
/// its usage hint. A host with no Stado at `HOST_PROGRAM` is left to the
/// prologue, which refuses it as `program_missing`.
fn verb_check() -> String {
    let verbs: Vec<String> = host_verbs()
        .iter()
        .map(|verb| crate::deploy::shlex_quote(verb))
        .collect();
    format!(
        r#"program="{HOST_PROGRAM}"
if [ -x "$program" ]; then
  for verb in {verbs}; do
    if ! said=$("$program" host local api $verb --help 2>&1 >/dev/null); then
      printf 'host_stado_lacks_verb %s does not declare host local api %s: %s\n' "$program" "$verb" "$said" >&2
      false
    fi
  done
fi
"#,
        verbs = verbs.join(" ")
    )
}

/// Restore the core object API without depending on the API being available.
///
/// Storage authority changes belong to the resident storage-root transaction.
/// This narrower repair shares its lock and never copies a backing store.
async fn recover_object_api_on_target(
    resolved: &ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    // The program reaches bash on stdin; the host's Stado holds the
    // storage-root transaction's flock for as long as it runs.
    let script = format!(
        r#"set -eu
{checks}if [ "$(uname)" = Linux ]; then decode=--decode; else decode=-D; fi
stado="$HOME/.stado/bin/stado"
[ -x "$stado" ] || stado="$(command -v stado)"
printf '%s' '{encoded}' | /usr/bin/base64 "$decode" |
  "$stado" host local run "$HOME/.stado/recovery/storage-root-reconcile.lock" -- /bin/bash"#,
        checks = verb_check(),
        encoded = STANDARD.encode(RECOVERY_PROGRAM),
    );
    let recovered = crate::deploy::host_channel::run_script(resolved, &script, runner)
        .await
        .map_err(CmdError::from)?;
    if !recovered.succeeded() {
        // The whole failure, not its last line: a host Stado that does not
        // declare a verb the program runs says which one above its usage
        // hint, and the hint alone told the operator nothing.
        return Err(CmdError::click(format!(
            "{}: object API recovery failed, the host's recovery program ran with its own Stado: \
             {}",
            resolved.name,
            crate::deploy::host_channel::whole_failure(&recovered)
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(recovered.stdout.trim().to_string())
}

/// Apply the declared object-API repair.
pub(crate) async fn apply_object_api_repair(target: &str) -> Result<Value, CmdError> {
    let resolved = crate::cli::canonical_host(target).await?;
    let detail =
        recover_object_api_on_target(&resolved, &crate::deploy::production_runner()).await?;
    Ok(json!({
        "target": resolved.name,
        "healthy": true,
        "detail": detail,
    }))
}

/// Apply the declared release-catalog ownership repair.
pub(crate) async fn apply_release_store_repair(
    target: &str,
    product: &str,
) -> Result<Value, CmdError> {
    if !crate::release_control::identifier(product) {
        return Err(CmdError::usage(
            "product must be a canonical release identifier",
        ));
    }
    let resolved = crate::cli::canonical_host(target).await?;
    let runner = crate::deploy::production_runner();
    // The host's own Stado does the bounded ownership repair; see
    // `stado host local repair`.
    let script = format!(
        "set -eu\nexec \"$HOME/.stado/bin/stado\" host local repair \
         --config \"${{STADO_CONFIG:-$HOME/.config/stado/config.json}}\" --product {}\n",
        crate::deploy::shlex_quote(product),
    );
    let repaired = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(CmdError::from)?;
    if !repaired.succeeded() {
        return Err(CmdError::click(format!(
            "{}: release store repair by the host's own Stado failed: {}",
            resolved.name,
            crate::deploy::host_channel::whole_failure(&repaired)
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let detail = repaired.stdout.trim();
    Ok(json!({
        "target": resolved.name,
        "status": "repaired",
        "scope": format!("stado://system/release-catalog/{product}.json"),
        "detail": detail,
    }))
}

/// Apply the agent's declared Skarbiec endpoint repair.
///
/// Derived, never invented: the value comes from
/// `service_directory.services.skarbiec.endpoints[<target>]`, and a host the
/// directory gives no endpoint is refused rather than pointed at a guess.
pub(crate) async fn apply_agent_skarbiec_repair(target: &str) -> Result<Value, CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let canonical = crate::cli::canonical_host(target).await?;
    let declared = document
        .pointer("/service_directory/services/skarbiec/endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(&canonical.name))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "the service directory declares no skarbiec endpoint for {}, so this host has \
                 no credential broker of its own to name. Declare one with `stado service \
                 directory endpoint skarbiec --target {} --url <url>`, or leave \
                 agent.skarbiec.url unset so the agent reads through the configured store \
                 client",
                canonical.name, canonical.name
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .to_string();

    let runner = crate::deploy::production_runner();
    let stdout = remote_config_output(&canonical, RemoteConfigAction::Show, &runner).await?;
    // An answer that is not JSON says nothing about the host's setting; read
    // as unset, the repair reported `previous: null` and rewrote it blind.
    let config: Value = serde_json::from_str(&stdout).map_err(|error| {
        CmdError::click(format!(
            "{}: the host's configuration answer is not JSON, so its agent.skarbiec.url cannot \
             be read: {error}",
            canonical.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let current = config
        .pointer("/resolved/agent_skarbiec_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let changed = current.as_deref() != Some(declared.as_str());
    if changed {
        write_host_config(&canonical.name, "agent.skarbiec.url", &declared).await?;
    }
    Ok(json!({
        "target": canonical.name,
        "declared": declared,
        "previous": current,
        "changed": changed,
    }))
}
