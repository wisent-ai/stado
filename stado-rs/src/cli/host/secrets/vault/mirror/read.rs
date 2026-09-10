use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::machine::releases::release_component;
use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::vault::mirror::skarbiec_tool_path;

/// [`remote_skarbiec_json`], optionally against a vault file other than the
/// target's live one.
///
/// The override exists for the sync preview and for nothing else: the only way
/// to say what a pull would change is to read the mirror as a vault, and
/// Skarbiec answers that question for whatever `SKARBIEC_VAULT_FILE` names.
/// The path is built from the target's own `$HOME`, never from an operator
/// argument, so this widens what Stado can read and not who can choose it.
pub(in crate::cli::host) async fn remote_skarbiec_json_at(
    target: &str,
    arguments: &[String],
    vault_relative: Option<&str>,
    token_source: Option<(&str, &str)>,
    token_file_name: Option<&str>,
) -> Result<(ComputeTarget, Value), CmdError> {
    let command = arguments
        .first()
        .map(String::as_str)
        .ok_or_else(|| CmdError::usage("a Skarbiec command is required"))?;
    let credential_host = credential_host(target).await?;
    let resolved = credential_host.target;
    let home = credential_host.home;
    let vault = credential_host.vault;
    let gnupg_home = credential_host.gnupg_home;
    let runner = crate::deploy::production_runner();
    let skarbiec = crate::cli::host::release_managed_skarbiec(&resolved, &runner, &home).await?;
    let vault_environment = match vault_relative {
        Some(relative) => format!("SKARBIEC_VAULT_FILE={home}/{relative}"),
        None => format!("SKARBIEC_VAULT_FILE={vault}"),
    };
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let tool_path = skarbiec_tool_path(&home);
    let token_file = if let Some(name) = token_file_name {
        release_component("token file name", name)?;
        let path = format!("{home}/.stado/{name}");
        let script = format!(
            r#"set -euo pipefail
umask 077
directory={directory}
destination={destination}
/bin/mkdir -p "$directory"
if [ -L "$destination" ]; then
  printf '%s\n' 'token file must not be a symlink' >&2
  exit 1
fi
if [ -e "$destination" ]; then
  if [ ! -f "$destination" ] || [ ! -s "$destination" ]; then
    printf '%s\n' 'token file must be a nonempty regular file' >&2
    exit 1
  fi
else
  pending="$destination.pending.$$"
  trap 'rm -f "$pending"' EXIT
  /usr/bin/openssl rand -hex 32 > "$pending"
  /bin/chmod 600 "$pending"
  if ! /bin/ln "$pending" "$destination"; then
    printf '%s\n' 'token file was created concurrently; retry using the persisted file' >&2
    exit 1
  fi
fi
"#,
            directory = crate::deploy::shlex_quote(&format!("{home}/.stado")),
            destination = crate::deploy::shlex_quote(&path),
        );
        let prepared = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !prepared.ok() {
            return Err(CmdError::click(format!(
                "{}: preparing token file {path} failed: {}",
                resolved.name,
                crate::deploy::host_channel::last_error_line(
                    &prepared,
                    "remote token file creation failed"
                )
            )));
        }
        Some(path)
    } else {
        None
    };
    let mut invocation = vec![
        "/usr/bin/env",
        tool_path.as_str(),
        gnupg_environment.as_str(),
        vault_environment.as_str(),
    ];
    let output = if let Some((item, field)) = token_source {
        invocation.extend(["/usr/bin/python3", "-", skarbiec.as_str(), item, field]);
        invocation.extend(arguments.iter().map(String::as_str));
        crate::deploy::host_channel::run_program_with_stdin(
            &resolved,
            &invocation,
            include_str!("../../../../../host_payloads/vault-token-from-item.py"),
            &runner,
        )
        .await
    } else {
        invocation.push(skarbiec.as_str());
        invocation.extend(arguments.iter().map(String::as_str));
        if let Some(path) = &token_file {
            invocation.extend(["--token-file", path.as_str()]);
        }
        crate::deploy::host_channel::run_program(&resolved, &invocation, &runner).await
    }
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        let retained = token_file
            .as_ref()
            .map(|path| format!("; bearer remains at {path} for a retry"))
            .unwrap_or_default();
        return Err(CmdError::click(format!(
            "{}: Skarbiec {command} failed: {}{retained}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    let mut report: Value = serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: Skarbiec {command} returned unreadable JSON: {error}",
            resolved.name
        ))
    })?;
    if let Some(path) = token_file {
        report["token_file"] = Value::String(path);
    }
    Ok((resolved, report))
}
