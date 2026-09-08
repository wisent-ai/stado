use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

/// Atomically replace one target-local verifier shadow with the authoritative
/// value, prove it, and record the source item's lifecycle.
#[allow(clippy::too_many_arguments)]
pub(super) async fn converge_item(
    source_lifecycles: &mut Vec<Value>,
    resolved: &ComputeTarget,
    runner: &crate::deploy::Runner,
    kind: &str,
    item: &str,
    skarbiec: &str,
    vault: &str,
    gnupg_home: &str,
    authoritative: &Value,
    target_items: &Value,
    target_owner: &str,
) -> Result<(), CmdError> {
    let source_entry = authoritative
        .get("items")
        .and_then(Value::as_object)
        .and_then(|entries| entries.get(item))
        .ok_or_else(|| {
            CmdError::click(format!(
                "authoritative {kind} verifier source item {item} is absent"
            ))
        })?;
    let source_management = source_entry
        .get("management")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::click(format!(
                "authoritative {kind} verifier source item {item} has no management metadata"
            ))
        })?;
    let mode = source_management
        .get("mode")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "owner" | "managed" | "external"))
        .ok_or_else(|| {
            CmdError::click(format!(
                "authoritative {kind} verifier source item {item} has no supported lifecycle mode"
            ))
        })?;
    let controller = source_management
        .get("controller")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "authoritative {kind} verifier source item {item} has no lifecycle controller"
            ))
        })?;
    let token = crate::credential_store::owner::read_string(item, "token").map_err(|error| {
        CmdError::click(format!(
            "cannot read authoritative {kind} verifier source item {item}: {error}"
        ))
    })?;
    let target_entry = target_items.as_array().and_then(|entries| {
        entries
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(item))
    });
    // Release shadows must be owned by the target vault. The
    // host-health item may itself be authoritative when the object API
    // runs beside the control-plane vault, so equality is sufficient.
    let shadow_owned = kind == "object"
        || target_entry
            .and_then(|entry| entry.get("management"))
            .and_then(Value::as_object)
            .is_some_and(|management| {
                management.get("mode").and_then(Value::as_str) == Some("owner")
                    && management.get("controller").and_then(Value::as_str) == Some(target_owner)
            });
    let compare_command = format!(
        "set -eu; expected=$(/bin/cat); actual=$(PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} get {} --field token); [ \"$actual\" = \"$expected\" ]",
        crate::deploy::shlex_quote(gnupg_home),
        crate::deploy::shlex_quote(vault),
        crate::deploy::shlex_quote(skarbiec),
        crate::deploy::shlex_quote(item),
    );
    let mut comparison = crate::deploy::host_channel::run_program_with_stdin(
        resolved,
        &["/bin/sh", "-c", &compare_command],
        &format!("{token}\n"),
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !shadow_owned || !comparison.ok() {
        let payload = serde_json::to_string(&serde_json::json!({
            "schema": "skarbiec.item.v2",
            "kind": "token",
            "fields": { "token": token },
            "context": {}
        }))?;
        let staging = format!("{vault}.stado-{kind}-verifier");
        let set_command = format!(
            "set -eu; live={}; staging={}; trap '/bin/rm -f \"$staging\"' EXIT HUP INT TERM; /bin/cp -p \"$live\" \"$staging\"; PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin GNUPGHOME={} SKARBIEC_VAULT_FILE=\"$staging\" {} reclaim {} >/dev/null 2>&1 || true; PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin GNUPGHOME={} SKARBIEC_VAULT_FILE=\"$staging\" {} rm {} >/dev/null 2>&1 || true; PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin GNUPGHOME={} SKARBIEC_VAULT_FILE=\"$staging\" {} set-json {} --type token >/dev/null; /bin/chmod 600 \"$staging\"; /bin/mv -f \"$staging\" \"$live\"; trap - EXIT HUP INT TERM",
            crate::deploy::shlex_quote(vault),
            crate::deploy::shlex_quote(&staging),
            crate::deploy::shlex_quote(gnupg_home),
            crate::deploy::shlex_quote(skarbiec),
            crate::deploy::shlex_quote(item),
            crate::deploy::shlex_quote(gnupg_home),
            crate::deploy::shlex_quote(skarbiec),
            crate::deploy::shlex_quote(item),
            crate::deploy::shlex_quote(gnupg_home),
            crate::deploy::shlex_quote(skarbiec),
            crate::deploy::shlex_quote(item),
        );
        let convergence_transport_error = match crate::deploy::host_channel::run_program_with_stdin(
            resolved,
            &["/bin/sh", "-c", &set_command],
            &payload,
            runner,
        )
        .await
        {
            Ok(converged) if converged.ok() => None,
            // A vault replacement can close the host channel after
            // the atomic move but before the shell reports success.
            // A nonzero transport-shaped result is therefore as
            // ambiguous as an I/O error: reconnect and prove the
            // target item before deciding whether the write failed.
            Ok(converged) => Some(
                crate::deploy::host_channel::last_error_line(
                    &converged,
                    "verifier shadow command ended before acknowledgement",
                )
                .to_string(),
            ),
            // Replacing the vault may invalidate the transport whose
            // credential came from that vault. Reconnect and judge the
            // item by its postcondition instead of repeating the write.
            Err(error) => Some(error.to_string()),
        };
        comparison = crate::deploy::host_channel::run_program_with_stdin(
            resolved,
            &["/bin/sh", "-c", &compare_command],
            &format!("{token}\n"),
            runner,
        )
        .await
        .map_err(|error| {
            let first = convergence_transport_error
                .as_deref()
                .unwrap_or("shadow write completed");
            CmdError::click(format!(
                "{}: cannot verify {kind} verifier shadow for {item} after {first}: {error}",
                resolved.name
            ))
        })?;
    }
    if !comparison.ok() {
        return Err(CmdError::click(format!(
            "{}: {kind} verifier shadow for {item} differs after reconciliation",
            resolved.name,
        )));
    }
    source_lifecycles.push(json!({
        "item": item,
        "mode": mode,
        "controller": controller,
        "readable": true,
    }));

    Ok(())
}
