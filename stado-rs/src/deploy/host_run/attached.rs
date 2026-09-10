//! Attached execution of one confined program: the host-side signal marker,
//! the forwarding of HUP/INT/TERM to the remote child, and the outcome the
//! caller sees.

use std::process::Stdio;

use serde::Serialize;
use tokio::io::AsyncReadExt;

use crate::deploy::{
    host_channel, production_runner, shlex_quote, host_access::ssh_key, CommandSpec, DeployError,
};
use crate::targets::ComputeTarget;

use super::{confined_file_prelude, SIGNAL_AREA, SIGNAL_TIMEOUT};

#[derive(Debug, Serialize)]
pub struct AttachedOutcome {
    pub target: String,
    pub program: String,
    pub arguments: Vec<String>,
    pub status: &'static str,
    pub exit_code: i32,
    pub forwarded_signals: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

fn attached_script(program: &str, arguments: &[String], token: &str) -> String {
    let mut script = String::from("set -uo pipefail\numask 077\n");
    script.push_str(&confined_file_prelude(program, "-x", "program"));
    script.push_str(&format!(
        r#"signal_root="$declared_home/{SIGNAL_AREA}"
for component in "$declared_home/.stado" "$declared_home/.stado/work" "$signal_root"; do
  if [ -L "$component" ]; then printf '%s\n' "signal marker ancestor is a symlink: $component" >&2; exit 64; fi
  if [ ! -e "$component" ]; then /bin/mkdir "$component" || exit 73; fi
  if [ ! -d "$component" ] || [ ! -O "$component" ]; then printf '%s\n' "signal marker ancestor is not an owned directory: $component" >&2; exit 64; fi
done
/bin/chmod 700 "$signal_root" || exit 73
marker="$signal_root/{token}"
temporary="$marker.$$"
child=''
cleanup() {{ /bin/rm -f -- "$temporary" "$marker"; }}
forward() {{ if [ -n "$child" ]; then /bin/kill "-$1" "$child" 2>/dev/null || :; fi; }}
trap cleanup EXIT
trap 'forward HUP' HUP
trap 'forward INT' INT
trap 'forward TERM' TERM
"#
    ));
    script.push_str("\"$path\"");
    for argument in arguments {
        script.push(' ');
        script.push_str(&shlex_quote(argument));
    }
    script.push_str(
        " <&0 &\nchild=$!\nprintf '%s\\n' \"$child\" > \"$temporary\" || exit 73\n/bin/chmod 600 \"$temporary\" || exit 73\n/bin/mv -f -- \"$temporary\" \"$marker\" || exit 73\nstatus=0\nwhile :; do\n  wait \"$child\"\n  status=$?\n  if ! /bin/kill -0 \"$child\" 2>/dev/null; then break; fi\ndone\nexit \"$status\"\n",
    );
    script
}

fn signal_script(token: &str, signal: &str) -> String {
    format!(
        r#"set -eu
signal_root="$HOME/{SIGNAL_AREA}"
marker="$signal_root/{token}"
for component in "$HOME/.stado" "$HOME/.stado/work" "$signal_root"; do
  [ ! -L "$component" ] || {{ printf '%s\n' "signal marker ancestor is a symlink: $component" >&2; exit 64; }}
  [ -d "$component" ] && [ -O "$component" ] || {{ printf '%s\n' "signal marker ancestor is not an owned directory: $component" >&2; exit 64; }}
done
[ ! -L "$marker" ] && [ -f "$marker" ] && [ -O "$marker" ] || {{ printf '%s\n' 'attached process signal marker is unavailable' >&2; exit 66; }}
IFS= read -r pid < "$marker"
case "$pid" in ''|*[!0-9]*) printf '%s\n' 'attached process signal marker has an invalid pid' >&2; exit 65 ;; esac
/bin/kill -0 "$pid" 2>/dev/null || {{ printf '%s\n' 'attached process has already exited' >&2; exit 66; }}
/bin/kill -{signal} "$pid"
"#
    )
}

async fn forward_signal(
    connection: Option<&str>,
    key: Option<&ssh_key::KeyFile>,
    token: &str,
    signal: &str,
) -> Result<(), DeployError> {
    let script = signal_script(token, signal);
    let argv = match (connection, key) {
        (None, None) => vec!["/bin/bash".to_string(), "-s".to_string()],
        (Some(destination), Some(key)) => {
            ssh_key::add_identity(host_channel::ssh_script_argv(destination), key)?
        }
        _ => {
            return Err(DeployError(
                "attached host channel is incomplete".to_string(),
            ))
        }
    };
    let output = production_runner()(CommandSpec {
        argv,
        stdin: Some(script),
        timeout: Some(SIGNAL_TIMEOUT),
    })
    .await
    .map_err(DeployError)?;
    if output.ok() {
        Ok(())
    } else {
        Err(DeployError(host_channel::last_error_line(
            &output,
            "the attached signal was not delivered",
        )))
    }
}

pub async fn run_attached(
    target: &ComputeTarget,
    program: &str,
    arguments: &[String],
    capture_output: bool,
) -> Result<AttachedOutcome, DeployError> {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let script = attached_script(program, arguments, &token);
    let runner = production_runner();
    let (argv, key, connection) = if host_channel::target_is_this_host(target) {
        (
            vec!["/bin/bash".to_string(), "-c".to_string(), script],
            None,
            None,
        )
    } else {
        let connection = host_channel::select_ssh_connection(target, &runner).await?;
        let key = ssh_key::materialize(target.channel_key()).await?;
        let mut argv = host_channel::ssh_options(connection.destination);
        argv.insert(1, "-T".to_string());
        // One quoted word for the login shell, and the script itself under the
        // same interpreter the local branch above uses. Pushing the script
        // bare made the account's login shell the interpreter, and this is the
        // only channel that did: every other one sends `/bin/bash -s` or
        // `/bin/sh -c` and keeps the script off the login shell's grammar.
        // On 2026-09-08 a real run against a zsh account reported
        // `exit_code: 1, status: failed` for an install that succeeded --
        // `status` is read-only in zsh, so the wrapper's own bookkeeping
        // assignment failed on a line the program never reached. Any shell
        // whose reserved names differ from bash's had the same power over
        // this outcome, which is why the repair is the interpreter and not
        // the variable name.
        argv.push(format!("/bin/bash -c {}", shlex_quote(&script)));
        let argv = ssh_key::add_identity(argv, &key)?;
        (argv, Some(key), Some(connection.destination.to_string()))
    };
    let (command, arguments_argv) = argv
        .split_first()
        .ok_or_else(|| DeployError("attached host channel is empty".to_string()))?;
    let mut command = tokio::process::Command::new(command);
    command
        .args(arguments_argv)
        .stdin(Stdio::inherit())
        .kill_on_drop(true);
    if capture_output {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| DeployError(error.to_string()))?;
    let stdout_reader = child.stdout.take().map(|mut stdout| {
        tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        })
    });
    let stderr_reader = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        })
    });
    let mut forwarded_signals = Vec::new();

    #[cfg(unix)]
    let status = {
        use tokio::signal::unix::{signal, SignalKind};
        let mut hangup =
            signal(SignalKind::hangup()).map_err(|error| DeployError(error.to_string()))?;
        let mut interrupt =
            signal(SignalKind::interrupt()).map_err(|error| DeployError(error.to_string()))?;
        let mut terminate =
            signal(SignalKind::terminate()).map_err(|error| DeployError(error.to_string()))?;
        loop {
            tokio::select! {
                status = child.wait() => break status.map_err(|error| DeployError(error.to_string()))?,
                received = hangup.recv() => {
                    if received.is_some() {
                        forward_signal(connection.as_deref(), key.as_ref(), &token, "HUP").await?;
                        forwarded_signals.push("SIGHUP".to_string());
                    }
                }
                received = interrupt.recv() => {
                    if received.is_some() {
                        forward_signal(connection.as_deref(), key.as_ref(), &token, "INT").await?;
                        forwarded_signals.push("SIGINT".to_string());
                    }
                }
                received = terminate.recv() => {
                    if received.is_some() {
                        forward_signal(connection.as_deref(), key.as_ref(), &token, "TERM").await?;
                        forwarded_signals.push("SIGTERM".to_string());
                    }
                }
            }
        }
    };
    #[cfg(not(unix))]
    let status = child
        .wait()
        .await
        .map_err(|error| DeployError(error.to_string()))?;

    let stdout = match stdout_reader {
        Some(reader) => Some(
            String::from_utf8_lossy(
                &reader
                    .await
                    .map_err(|error| DeployError(error.to_string()))?
                    .map_err(|error| DeployError(error.to_string()))?,
            )
            .into_owned(),
        ),
        None => None,
    };
    let stderr = match stderr_reader {
        Some(reader) => Some(
            String::from_utf8_lossy(
                &reader
                    .await
                    .map_err(|error| DeployError(error.to_string()))?
                    .map_err(|error| DeployError(error.to_string()))?,
            )
            .into_owned(),
        ),
        None => None,
    };
    drop(key);
    let exit_code = status.code().unwrap_or(1);
    Ok(AttachedOutcome {
        target: target.name.clone(),
        program: program.to_string(),
        arguments: arguments.to_vec(),
        status: if status.success() { "exited" } else { "failed" },
        exit_code,
        forwarded_signals,
        stdout,
        stderr,
    })
}
