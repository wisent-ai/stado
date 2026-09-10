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
) -> Result<(), CmdError> {
    let target = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let stdout = remote_config_output(&target, action, &crate::deploy::production_runner()).await?;
    print!("{stdout}");
    Ok(())
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
/// The incident: the Mac mini's agent unit was re-declared with a
/// `STADO_CONFIG` naming a config that set `wc_storage_backend: "local"`, so
/// the agent published its capacity into an on-disk store on that machine and
/// nothing in the fleet ever read it. `host config-show` could see that field
/// the whole time; nothing that judged the host asked it.
pub(crate) async fn remote_config_output(
    target: &ComputeTarget,
    action: RemoteConfigAction<'_>,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    let action = match action {
        RemoteConfigAction::Show => "\"$binary\" config show".to_string(),
        RemoteConfigAction::Set { key, value } => format!(
            "key=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
             value=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
             \"$binary\" config migrate\n\
             \"$binary\" config set \"$key\" \"$value\"\n\
             \"$binary\" config show",
            STANDARD.encode(key.as_bytes()),
            STANDARD.encode(value.as_bytes())
        ),
    };
    let script = format!("{CONFIG_SCRIPT_PREFIX}{action}\n");
    let output = crate::deploy::host_channel::run_script_with_timeout(
        target,
        &script,
        std::time::Duration::from_secs(60),
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        let detail = output.detail().trim().to_string();
        return Err(CmdError::click(if detail.is_empty() {
            format!(
                "host configuration command on {} exited with code {} and produced no output",
                target.name, output.code
            )
        } else {
            detail
        }));
    }
    Ok(output.stdout)
}
