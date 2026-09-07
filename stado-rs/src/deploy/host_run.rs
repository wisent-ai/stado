//! Build and attached execution inside one target account's managed run area.
//!
//! The caller names only a registry target and paths below that target's
//! `$HOME/.stado/work/runs`. Paths are checked once before host resolution for
//! obvious misuse and again on the host against the login account's real home.
//! The host-side check refuses symlinked ancestors, foreign ownership, and a
//! file of the wrong kind before a compiler or program starts.

use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt;

use super::{host_channel, py_str_repr, shlex_quote, ssh_key, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

const RUN_AREA: &str = ".stado/work/runs";
const SIGNAL_AREA: &str = ".stado/work/run-signals";
const BUILD_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(20);
const PATH_REFUSAL: &str =
    "must be an absolute path below the target account's $HOME/.stado/work/runs, with no '.' or '..' component";

#[derive(Debug, Serialize)]
pub struct BuildOutcome {
    pub target: String,
    pub manifest_path: String,
    pub binary: String,
    pub status: &'static str,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

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

#[derive(Debug, Serialize)]
pub struct RemoveRunDirectoryOutcome {
    pub target: String,
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl RemoveRunDirectoryOutcome {
    pub fn succeeded(&self) -> bool {
        self.status == "removed" || self.status == "absent"
    }

    pub fn failure_sentence(&self) -> String {
        format!(
            "{}: {} {}{}",
            self.target,
            self.path,
            self.status,
            self.detail
                .as_ref()
                .filter(|detail| !detail.is_empty())
                .map(|detail| format!(" — {detail}"))
                .unwrap_or_default()
        )
    }
}

/// Reject an argument that cannot possibly be inside the target's managed run
/// area before registry or network access. The host still binds the candidate
/// to its own `$HOME`; this lexical check does not guess that home locally.
pub fn validate_run_descendant(path: &str) -> Result<(), String> {
    let components = std::path::Path::new(path).components().collect::<Vec<_>>();
    let ordinary = components.iter().skip(1).all(|component| {
        matches!(component, std::path::Component::Normal(_))
    });
    let managed = path
        .split_once(&format!("/{RUN_AREA}/"))
        .is_some_and(|(home, relative)| !home.is_empty() && !relative.is_empty());
    if path.starts_with('/') && !path.contains('\0') && ordinary && managed {
        Ok(())
    } else {
        Err(format!("path {} {PATH_REFUSAL}", py_str_repr(path)))
    }
}

/// A recursive delete addresses one complete run, never the shared run root or
/// one nested subtree. Build and execution accept deeper descendants.
pub fn validate_run_directory(path: &str) -> Result<(), String> {
    validate_run_descendant(path)?;
    let relative = path
        .split_once(&format!("/{RUN_AREA}/"))
        .map(|(_, relative)| relative)
        .unwrap_or_default();
    let safe_name = !relative.is_empty()
        && !relative.contains('/')
        && relative
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if safe_name {
        Ok(())
    } else {
        Err(format!(
            "run directory {} must be one direct, safely named child of the target account's $HOME/{RUN_AREA}",
            py_str_repr(path)
        ))
    }
}

pub fn validate_binary_name(binary: &str) -> Result<(), String> {
    let safe = binary
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && binary
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if safe {
        Ok(())
    } else {
        Err(format!(
            "binary name {} must start with a letter or number and contain only letters, numbers, dot, dash, or underscore",
            py_str_repr(binary)
        ))
    }
}

pub fn validate_arguments(arguments: &[String]) -> Result<(), String> {
    if let Some(argument) = arguments.iter().find(|argument| argument.contains('\0')) {
        Err(format!(
            "program argument {} carries a NUL byte",
            py_str_repr(argument)
        ))
    } else {
        Ok(())
    }
}

/// Host-side confinement shared by build and attached execution. `kind_test`
/// is fixed by this module (`-f` or `-x`), never caller text.
fn confined_file_prelude(path: &str, kind_test: &str, role: &str) -> String {
    let path = shlex_quote(path);
    format!(
        r#"path={path}
declared_home=${{HOME%/}}
case "$path" in
  "$declared_home/{RUN_AREA}/"*) ;;
  *) printf '%s\n' '{role} path is outside the managed run area: expected $HOME/{RUN_AREA}/...' >&2; exit 64 ;;
esac
for component in "$declared_home/.stado" "$declared_home/.stado/work" "$declared_home/{RUN_AREA}"; do
  if [ -L "$component" ]; then printf '%s\n' "managed run ancestor is a symlink: $component" >&2; exit 64; fi
  if [ ! -d "$component" ]; then printf '%s\n' "managed run ancestor is not a directory: $component" >&2; exit 64; fi
  if [ ! -O "$component" ]; then printf '%s\n' "managed run ancestor is not owned by this account: $component" >&2; exit 64; fi
done
if [ -L "$path" ]; then printf '%s\n' '{role} path is a symlink' >&2; exit 64; fi
if [ ! {kind_test} "$path" ]; then printf '%s\n' '{role} path is not an eligible file' >&2; exit 66; fi
if [ ! -O "$path" ]; then printf '%s\n' '{role} path is not owned by this account' >&2; exit 64; fi
parent=${{path%/*}}
physical_home=$(cd -P -- "$declared_home" && /bin/pwd -P) || exit 64
physical_parent=$(cd -P -- "$parent" && /bin/pwd -P) || exit 64
relative_parent=${{parent#"$declared_home"/}}
if [ "$relative_parent" = "$parent" ] || [ "$physical_parent" != "$physical_home/$relative_parent" ]; then
  printf '%s\n' '{role} path crosses a symlinked ancestor outside the managed run area' >&2
  exit 64
fi
"#
    )
}

pub async fn build(
    target: &ComputeTarget,
    manifest_path: &str,
    binary: &str,
    runner: &Runner,
) -> Result<BuildOutcome, DeployError> {
    let mut script = String::from("set -uo pipefail\numask 077\n");
    script.push_str(&confined_file_prelude(
        manifest_path,
        "-f",
        "manifest",
    ));
    script.push_str("cargo=''\n");
    for candidate in super::host_exec::cargo_candidates() {
        let candidate = if let Some(relative) = candidate.strip_prefix("~/") {
            format!("\"$HOME/{}\"", shlex_quote(relative))
        } else {
            shlex_quote(candidate)
        };
        script.push_str(&format!(
            "if [ -z \"$cargo\" ] && [ -x {candidate} ]; then cargo={candidate}; fi\n"
        ));
    }
    script.push_str(
        "if [ -z \"$cargo\" ]; then printf '%s\\n' 'Cargo is not installed at a Stado-approved path' >&2; exit 69; fi\n",
    );
    script.push_str("export PATH=\"${cargo%/*}:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin\"\n");
    script.push_str(&format!(
        "\"$cargo\" build --locked --release --manifest-path \"$path\" --bin {}\n",
        shlex_quote(binary)
    ));

    let output = host_channel::run_script_with_timeout(target, &script, BUILD_TIMEOUT, runner).await?;
    Ok(BuildOutcome {
        target: target.name.clone(),
        manifest_path: manifest_path.to_string(),
        binary: binary.to_string(),
        status: if output.ok() { "built" } else { "failed" },
        exit_code: output.code,
        stdout: output.stdout,
        stderr: output.stderr,
    })
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
        _ => return Err(DeployError("attached host channel is incomplete".to_string())),
    };
    let output = super::production_runner()(CommandSpec {
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
    let runner = super::production_runner();
    let (argv, key, connection) = if host_channel::target_is_this_host(target) {
        (
            vec!["/bin/bash".to_string(), "-c".to_string(), script],
            None,
            None,
        )
    } else {
        let connection = host_channel::select_ssh_connection(target, &runner).await?;
        let key = ssh_key::materialize(&target.name).await?;
        let mut argv = host_channel::ssh_options(connection.destination);
        argv.insert(1, "-T".to_string());
        argv.push(script);
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
    let mut child = command.spawn().map_err(|error| DeployError(error.to_string()))?;
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
        let mut hangup = signal(SignalKind::hangup()).map_err(|error| DeployError(error.to_string()))?;
        let mut interrupt = signal(SignalKind::interrupt()).map_err(|error| DeployError(error.to_string()))?;
        let mut terminate = signal(SignalKind::terminate()).map_err(|error| DeployError(error.to_string()))?;
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
    let status = child.wait().await.map_err(|error| DeployError(error.to_string()))?;

    let stdout = match stdout_reader {
        Some(reader) => Some(String::from_utf8_lossy(
            &reader
                .await
                .map_err(|error| DeployError(error.to_string()))?
                .map_err(|error| DeployError(error.to_string()))?,
        )
        .into_owned()),
        None => None,
    };
    let stderr = match stderr_reader {
        Some(reader) => Some(String::from_utf8_lossy(
            &reader
                .await
                .map_err(|error| DeployError(error.to_string()))?
                .map_err(|error| DeployError(error.to_string()))?,
        )
        .into_owned()),
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

pub async fn remove_run_directory(
    target: &ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<RemoveRunDirectoryOutcome, DeployError> {
    let quoted = shlex_quote(path);
    let script = format!(
        r#"set -u
path={quoted}
report() {{ printf 'STADO_REMOVE_RUN_DIRECTORY\t%s\t%s\n' "$1" "$2"; }}
declared_home=${{HOME%/}}
case "$path" in
  "$declared_home/{RUN_AREA}/"*) ;;
  *) report refused 'outside the managed run area; expected $HOME/{RUN_AREA}/RUN'; exit 0 ;;
esac
relative=${{path#"$declared_home/{RUN_AREA}/"}}
case "$relative" in ''|*/*) report refused 'a recursive removal must name one direct child of the managed run area'; exit 0 ;; esac
if [ ! -e "$path" ] && [ ! -L "$path" ]; then report absent ''; exit 0; fi
for component in "$declared_home/.stado" "$declared_home/.stado/work" "$declared_home/{RUN_AREA}"; do
  if [ -L "$component" ]; then report refused "managed run ancestor is a symlink: $component"; exit 0; fi
  if [ ! -d "$component" ]; then report refused "managed run ancestor is not a directory: $component"; exit 0; fi
  if [ ! -O "$component" ]; then report refused "managed run ancestor is not owned by this account: $component"; exit 0; fi
done
if [ -L "$path" ]; then report refused 'run directory is a symlink'; exit 0; fi
if [ ! -d "$path" ]; then report refused 'run path is not a directory'; exit 0; fi
if [ ! -O "$path" ]; then report refused 'run directory is not owned by this account'; exit 0; fi
physical_home=$(cd -P -- "$declared_home" && /bin/pwd -P) || {{ report refused 'target home could not be resolved'; exit 0; }}
physical_parent=$(cd -P -- "${{path%/*}}" && /bin/pwd -P) || {{ report refused 'run parent could not be resolved'; exit 0; }}
if [ "$physical_parent" != "$physical_home/{RUN_AREA}" ]; then report refused 'run directory crosses a symlinked ancestor outside the managed run area'; exit 0; fi
/bin/rm -rf -- "$path"
if [ -e "$path" ] || [ -L "$path" ]; then report failed 'rm returned and the run directory is still present'; else report removed ''; fi
"#
    );
    let output = host_channel::run_script_with_timeout(
        target,
        &script,
        Duration::from_secs(5 * 60),
        runner,
    )
    .await?;
    let (status, detail) = output
        .stdout
        .lines()
        .find_map(|line| {
            let fields = host_channel::marker_fields(line);
            (fields.first() == Some(&"STADO_REMOVE_RUN_DIRECTORY") && fields.len() >= 3)
                .then(|| (fields[1].to_string(), (!fields[2].is_empty()).then(|| fields[2].to_string())))
        })
        .ok_or_else(|| {
            DeployError(format!(
                "{}: the host answered without a run-directory removal report: {}",
                target.name,
                host_channel::last_error_line(&output, "no marker in output")
            ))
        })?;
    Ok(RemoveRunDirectoryOutcome {
        target: target.name.clone(),
        path: path.to_string(),
        status,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_paths_are_bounded_to_one_managed_tree() {
        assert!(validate_run_descendant("/Users/dev/.stado/work/runs/abc/src/Cargo.toml").is_ok());
        assert!(validate_run_descendant("/tmp/Cargo.toml").is_err());
        assert!(validate_run_descendant("/Users/dev/.stado/work/runs/../secret").is_err());
        assert!(validate_run_directory("/Users/dev/.stado/work/runs/abc").is_ok());
        assert!(validate_run_directory("/Users/dev/.stado/work/runs/abc/src").is_err());
    }
}
