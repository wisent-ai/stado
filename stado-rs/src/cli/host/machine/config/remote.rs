use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::cli::CmdError;
use crate::deploy::{CommandOutput, Runner};
use crate::targets::ComputeTarget;

const CONFIG_SCRIPT_PREFIX: &str = "\
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
    Unset { key: &'a str },
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
    // The host's native JSON document, rendered as local `config show` text.
    let document: serde_json::Value = serde_json::from_str(&stdout).map_err(|error| {
        CmdError::click(format!(
            "{} answered the configuration read with something that is not JSON ({error}): {}",
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
    let (operation, command) = match action {
        RemoteConfigAction::Show => return read_configuration(target, runner).await,
        RemoteConfigAction::Set { key, value } => (
            format!("config set {key}"),
            format!(
                "key=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
                 value=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
                 \"$binary\" config set \"$key\" \"$value\"",
                STANDARD.encode(key.as_bytes()),
                STANDARD.encode(value.as_bytes())
            ),
        ),
        RemoteConfigAction::Unset { key } => (
            format!("config unset {key}"),
            format!(
                "key=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
                 \"$binary\" config unset \"$key\"",
                STANDARD.encode(key.as_bytes())
            ),
        ),
    };
    let output = configuration_command(target, &command, &operation, runner).await?;
    if !output.ok() {
        return Err(command_refusal(target, &operation, &output));
    }
    // Mutations run once. Their acknowledgement is not part of the JSON document.
    read_configuration(target, runner).await.map_err(|error| {
        CmdError::click(format!(
            "{operation} completed on {} (exit 0), but configuration readback failed: \
             {error}; mutation output: {}",
            target.name,
            output.detail().trim()
        ))
    })
}

async fn configuration_command(
    target: &ComputeTarget,
    command: &str,
    operation: &str,
    runner: &Runner,
) -> Result<CommandOutput, CmdError> {
    let script = format!("{CONFIG_SCRIPT_PREFIX}{command}\n");
    crate::deploy::host_channel::run_script(target, &script, runner)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "cannot run {operation} on {} through its host channel: {error}",
                target.name
            ))
        })
}

fn command_refusal(target: &ComputeTarget, operation: &str, output: &CommandOutput) -> CmdError {
    let detail = output.detail();
    CmdError::click(format!(
        "cannot run {operation} on {} using ~/.stado/bin/stado (exit {}): {}",
        target.name,
        output.code,
        if detail.trim().is_empty() {
            "no output"
        } else {
            detail.trim()
        }
    ))
}

async fn read_configuration(target: &ComputeTarget, runner: &Runner) -> Result<String, CmdError> {
    let explicit = configuration_command(
        target,
        "\"$binary\" config show --json",
        "config show --json",
        runner,
    )
    .await?;
    if explicit.ok() {
        return Ok(explicit.stdout);
    }
    let explicit_error = command_refusal(target, "config show --json", &explicit);
    if explicit.code != 2 {
        return Err(explicit_error);
    }
    // Earlier installed CLIs emit the same machine document without the flag.
    // Only a usage refusal negotiates that form; transport and execution failures do not.
    let implicit = configuration_command(target, "\"$binary\" config show", "config show", runner)
        .await
        .map_err(|error| CmdError::click(format!("{explicit_error}; {error}")))?;
    if !implicit.ok() {
        return Err(CmdError::click(format!(
            "{explicit_error}; {}",
            command_refusal(target, "config show", &implicit)
        )));
    }
    let document: serde_json::Value = serde_json::from_str(&implicit.stdout).map_err(|error| {
        CmdError::click(format!(
            "{explicit_error}; {} config show exited 0 but did not return JSON: {error}; output: {}",
            target.name, implicit.stdout.trim()
        ))
    })?;
    if document
        .get("file")
        .and_then(serde_json::Value::as_str)
        .is_none_or(str::is_empty)
        || !document
            .get("resolved")
            .is_some_and(serde_json::Value::is_object)
    {
        return Err(CmdError::click(format!(
            "{explicit_error}; {} config show exited 0 but returned an invalid configuration \
             document: expected a nonempty file string and resolved object; output: {}",
            target.name,
            implicit.stdout.trim()
        )));
    }
    Ok(implicit.stdout)
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
    remote_stado(target, "", arguments).await
}

/// [`remote_stado_output`] for an operation that builds on the host, such as
/// a forwarded `stado product install`: the host's toolchain is put on `PATH`
/// first, found the way `stado host build` finds Cargo
/// ([`crate::deploy::host_exec::cargo_candidates`]). A login shell is not what
/// the host channel runs, so rustup's `~/.cargo/bin` and Homebrew's prefixes
/// are otherwise absent and a source build fails at its first `cargo`. Nothing
/// is refused here when Cargo is missing: a product that builds without it
/// does not need it, and one that does names the missing program itself.
pub(crate) async fn remote_stado_build_output(
    target: &str,
    arguments: &[&str],
) -> Result<String, CmdError> {
    let mut prelude = String::from("cargo=''\n");
    for candidate in crate::deploy::host_exec::cargo_candidates() {
        let candidate = match candidate.strip_prefix("~/") {
            Some(relative) => format!("\"$HOME/{}\"", crate::deploy::shlex_quote(relative)),
            None => crate::deploy::shlex_quote(candidate),
        };
        prelude.push_str(&format!(
            "if [ -z \"$cargo\" ] && [ -x {candidate} ]; then cargo={candidate}; fi\n"
        ));
    }
    prelude.push_str(
        "export PATH=\"${cargo:+${cargo%/*}:}$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:$PATH\"\n",
    );
    remote_stado(target, &prelude, arguments).await
}

async fn remote_stado(target: &str, prelude: &str, arguments: &[&str]) -> Result<String, CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut script = CONFIG_SCRIPT_PREFIX.to_string();
    script.push_str(prelude);
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
