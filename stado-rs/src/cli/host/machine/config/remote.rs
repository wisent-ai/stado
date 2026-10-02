use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

pub(super) const CONFIG_SCRIPT_PREFIX: &str = "\
set -euo pipefail\n\
case \"$(/usr/bin/uname -s)\" in Darwin) decode=-D ;; *) decode=--decode ;; esac\n\
export STADO_CONFIG=\"$HOME/.config/stado/config.json\"\n\
binary=\"$HOME/.stado/bin/stado\"\n\
if ! test -x \"$binary\"; then\n\
  printf 'cannot read host configuration: %s is missing or not executable; configuration %s\\n' \"$binary\" \"$STADO_CONFIG\" >&2\n\
  exit 1\n\
fi\n";

pub(crate) enum RemoteConfigAction<'a> {
    Show,
    Set { key: &'a str, value: &'a str },
}

pub(super) async fn remote_config(
    target: &str,
    action: RemoteConfigAction<'_>,
    json: bool,
) -> Result<(), CmdError> {
    let target = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let stdout = remote_config_output(&target, action, &crate::deploy::production_runner()).await?;
    if json {
        print!("{stdout}");
        return Ok(());
    }
    // The host's own `config show --json` document, as the same
    // `key: value` lines a local `config show` prints.
    let document: serde_json::Value = serde_json::from_str(&stdout).map_err(|error| {
        CmdError::click(format!(
            "{} answered `config show --json` with something that is not JSON ({error}): {}",
            target.name,
            stdout.trim()
        ))
    })?;
    let mut lines = serde_json::Map::new();
    lines.insert("file".into(), document["file"].clone());
    if let Some(resolved) = document["resolved"].as_object() {
        lines.extend(resolved.clone());
    }
    crate::cli::print_answer(&serde_json::Value::Object(lines), false)
}

/// One Stado configuration action on a fleet host — the effective configuration its
/// own installed binary resolves, from its own `STADO_CONFIG` — returned
/// instead of printed.
///
/// Factored out of [`remote_config`] so [`crate::deploy::host_gates`] can ask
/// a host which storage backend its queue agent is bound to without a second
/// remote script existing for the same question. Two scripts reading one
/// host's configuration would eventually read it two different ways — a
/// different `STADO_CONFIG`, a different binary, a different `HOME` — and the
/// answer that matters here is exactly "what does the config the services
/// consume say", which is what this one already asks.
///
/// A successful device-local capacity write does not prove fleet publication.
/// Host gates inspect the configuration the host consumes, rather than infer
/// storage reach from a running process or an earlier capacity record.
pub(crate) async fn remote_config_output(
    target: &ComputeTarget,
    action: RemoteConfigAction<'_>,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    let operation = match &action {
        RemoteConfigAction::Show => "read configuration (`config show --json`)",
        RemoteConfigAction::Set { .. } => {
            "set configuration and read it back (`config set`, then `config show --json`)"
        }
    };
    let action = match action {
        RemoteConfigAction::Show => "\"$binary\" config show --json".to_string(),
        RemoteConfigAction::Set { key, value } => format!(
            "key=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
             value=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
             \"$binary\" config set \"$key\" \"$value\"\n\
             \"$binary\" config show --json",
            STANDARD.encode(key.as_bytes()),
            STANDARD.encode(value.as_bytes())
        ),
    };
    let script = format!("{CONFIG_SCRIPT_PREFIX}{action}\n");
    let output = crate::deploy::host_channel::run_script(target, &script, runner)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "cannot {operation} on {} through its host channel: {error}",
                target.name
            ))
        })?;
    if !output.ok() {
        let detail = output.detail();
        return Err(CmdError::click(format!(
            "cannot {operation} on {} using ~/.stado/bin/stado (exit {}): {}",
            target.name,
            output.code,
            if detail.trim().is_empty() {
                "no output"
            } else {
                detail.trim()
            }
        )));
    }
    Ok(output.stdout)
}

/// Run the host's own installed Stado with `arguments`, each carried base64
/// encoded and decoded into its own argv word, and return its output. The
/// command runs until it exits on the host; its exit code and output are the
/// answer.
///
/// A release client that holds no vault cannot reconcile a verifier: the
/// reconciliation reads the authoritative publisher items from the vault on
/// the machine it runs on. `declare-publisher` therefore runs it on the vault
/// owner through this, with the owner's own binary and configuration.
pub(crate) async fn remote_stado_output(
    target: &str,
    arguments: &[&str],
) -> Result<String, CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut script = CONFIG_SCRIPT_PREFIX.to_string();
    let mut words = Vec::with_capacity(arguments.len());
    for (index, argument) in arguments.iter().enumerate() {
        script.push_str(&format!(
            "a{index}=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n",
            STANDARD.encode(argument.as_bytes())
        ));
        words.push(format!("\"$a{index}\""));
    }
    script.push_str(&format!("\"$binary\" {}\n", words.join(" ")));
    let output = crate::deploy::host_channel::run_script_to_completion(
        &resolved,
        &script,
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        let detail = output.detail().trim().to_string();
        return Err(CmdError::click(format!(
            "stado {} on {} failed (exit {}): {}",
            arguments.join(" "),
            resolved.name,
            output.code,
            if detail.is_empty() {
                "no output"
            } else {
                &detail
            }
        )));
    }
    Ok(output.stdout)
}
