//! Moving a unit from its LaunchAgent to the launchd daemon of the same
//! label.
//!
//! `service ensure` installs a unit in the system domain when the plan forces
//! it ([`ensure_unit_path`]). The same label may still be loaded as a
//! LaunchAgent of the login, running the same program on the same port, and
//! a daemon started beside it fights it for the port and loses, while the
//! agent goes on running where macOS asks the person at the screen. These
//! steps take the agent out of the way and give it back: the agent is booted
//! out and its file kept until the daemon answers, then the file is removed;
//! when the daemon does not answer, the daemon is booted out, its file
//! removed, and the agent loaded again from the file that was kept.

use crate::deploy::service::*;

/// What [`agent_twin`] reads: whether this login holds an agent file or a
/// loaded agent job for the label.
const TWIN_SCRIPT: &str = r#"set -u
label=@LABEL@
report() { printf 'STADO_TWIN\t%s\t%s\t%s\n' "$1" "$2" "$3"; }
if [ "$(/usr/bin/uname -s)" != Darwin ]; then
  report absent '' ''
  exit 0
fi
uid=$(/usr/bin/id -u)
path="$HOME/@USER_LAUNCHD_AGENTS@/$label.plist"
loaded=''
for domain in "gui/$uid" "user/$uid"; do
  if /bin/launchctl print "$domain/$label" >/dev/null 2>&1; then
    loaded="$domain"
    break
  fi
done
if [ -f "$path" ]; then
  report present "$path" "$loaded"
elif [ -n "$loaded" ]; then
  report present '' "$loaded"
else
  report absent "$path" ''
fi
"#;

/// Removes one file and proves it gone; `@SUDO@` is `/usr/bin/sudo -n` for a
/// file root owns.
const REMOVE_SCRIPT: &str = r#"set -u
path=@PATH@
report() { printf 'STADO_TWIN_REMOVE\t%s\t%s\n' "$1" "$2"; }
if [ ! -e "$path" ]; then
  report absent "$path"
  exit 0
fi
detail=$(@SUDO@/bin/rm -f "$path" 2>&1)
if [ -e "$path" ]; then
  report refused "$(printf '%s' "rm -f $path left it in place: ${detail:-no detail}" | /usr/bin/tr '\t\r\n' '   ')"
else
  report removed "$path"
fi
"#;

/// Asks `sudo -n -l` whether root's steps of a move into the system domain
/// are granted without a password: placing the plist, loading it, booting it
/// out, and removing it on the way back. Runs nothing as root.
const GRANTS_SCRIPT: &str = r#"set -u
label=@LABEL@
daemon="@LAUNCHD_DAEMONS@/$label.plist"
check() {
  if said=$(/usr/bin/sudo -n -l "$@" 2>&1); then
    printf 'STADO_GRANT\tgranted\t%s\t\n' "$*"
  else
    printf 'STADO_GRANT\trefused\t%s\t%s\n' "$*" "$(printf '%s' "$said" | /usr/bin/tr '\t\r\n' '   ')"
  fi
}
check /usr/bin/install -m 644 -o root -g wheel "$HOME/.stado/$label.plist" "$daemon"
check /bin/launchctl bootstrap system "$daemon"
check /bin/launchctl bootout "system/$label"
check /bin/rm -f "$daemon"
printf 'STADO_GRANT_END\n'
"#;

/// One privileged step of a move into the system domain `sudo -n` refused,
/// with sudo's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedStep {
    pub command: String,
    pub said: String,
}

/// The privileged steps of moving `label` into the system domain on `target`
/// that `sudo -n` would refuse; empty when every one is granted.
pub async fn refused_daemon_steps(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<Vec<RefusedStep>, DeployError> {
    validate_unit_id(label)?;
    let script = with_unit_directories(GRANTS_SCRIPT)
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?));
    let output = host_channel::run_script(target, &script, runner).await?;
    if marker(&output.stdout, "STADO_GRANT_END").is_none() {
        return Err(DeployError::unreachable(format!(
            "{}: the read of root's grants ended without its end line: {}",
            target.name,
            host_channel::last_error_line(&output, "the host said nothing")
        )));
    }
    Ok(output
        .stdout
        .lines()
        .filter_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_GRANT", "refused", command, said] => Some(RefusedStep {
                command: format!("sudo -n {command}"),
                said: said.to_string(),
            }),
            _ => None,
        })
        .collect())
}

/// The LaunchAgent a login holds for a label the plan installs as a daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTwin {
    /// The agent's unit file, when the login still has one.
    pub path: Option<String>,
    /// The per-login domain launchd holds the agent job in, when loaded.
    pub loaded: Option<String>,
}

impl AgentTwin {
    /// The agent in the words a report prints.
    pub fn describe(&self, label: &str) -> String {
        match (&self.path, &self.loaded) {
            (Some(path), Some(domain)) => format!("the LaunchAgent {path} loaded in {domain}"),
            (Some(path), None) => format!("the LaunchAgent {path}, not loaded"),
            (None, Some(domain)) => format!("{domain}/{label}, loaded with no file"),
            (None, None) => format!("no LaunchAgent for {label}"),
        }
    }
}

fn marker<'a>(stdout: &'a str, name: &str) -> Option<Vec<&'a str>> {
    stdout
        .lines()
        .map(host_channel::marker_fields)
        .find(|fields| fields.first() == Some(&name))
}

/// The agent `target`'s login holds for `label`, or `None`.
pub async fn agent_twin(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<Option<AgentTwin>, DeployError> {
    validate_unit_id(label)?;
    let script = with_unit_directories(TWIN_SCRIPT)
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.succeeded() {
        return Err(DeployError::unreachable(host_channel::last_error_line(
            &output,
            "the read of the agent beside the daemon did not complete",
        )));
    }
    let Some(fields) = marker(&output.stdout, "STADO_TWIN") else {
        return Err(DeployError::unreachable(format!(
            "{}: the read of the LaunchAgent for {label} reported nothing",
            target.name
        )));
    };
    match fields.as_slice() {
        ["STADO_TWIN", "absent", ..] => Ok(None),
        ["STADO_TWIN", "present", path, loaded] => Ok(Some(AgentTwin {
            path: (!path.is_empty()).then(|| path.to_string()),
            loaded: (!loaded.is_empty()).then(|| loaded.to_string()),
        })),
        other => Err(DeployError::unreachable(format!(
            "{}: the read of the LaunchAgent for {label} answered {other:?}, which it does not \
             write",
            target.name
        ))),
    }
}

/// Boot the agent out of its per-login domain, keeping its file.
pub async fn stop_agent_twin(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    let (state, detail) = bootout_label(target, label, BootoutScope::User, runner).await?;
    if state.gone() {
        Ok(format!("{state} ({detail})"))
    } else {
        Err(DeployError::unreachable(format!(
            "{}: the LaunchAgent {label} could not be stopped before its daemon starts: {detail}",
            target.name
        )))
    }
}

async fn remove(
    target: &ComputeTarget,
    path: &str,
    privileged: bool,
    runner: &Runner,
) -> Result<String, DeployError> {
    let script = REMOVE_SCRIPT
        .replace("@PATH@", &format!("\"{}\"", quote_unit_path(path)?))
        .replace("@SUDO@", if privileged { "/usr/bin/sudo -n " } else { "" });
    let output = host_channel::run_script(target, &script, runner).await?;
    match marker(&output.stdout, "STADO_TWIN_REMOVE").as_deref() {
        Some(["STADO_TWIN_REMOVE", "removed" | "absent", said]) => Ok(said.to_string()),
        Some(["STADO_TWIN_REMOVE", _, said]) => {
            Err(DeployError::unreachable(format!("{}: {said}", target.name)))
        }
        _ => Err(DeployError::unreachable(host_channel::last_error_line(
            &output,
            "the removal reported nothing",
        ))),
    }
}

/// Remove the agent's file once its daemon answers.
pub async fn remove_agent_twin(
    target: &ComputeTarget,
    twin: &AgentTwin,
    runner: &Runner,
) -> Result<String, DeployError> {
    match &twin.path {
        Some(path) => remove(target, path, false, runner)
            .await
            .map(|path| format!("removed {path}")),
        None => Ok("the agent had no file to remove".to_string()),
    }
}

/// Put the agent back after its daemon did not answer: boot the daemon out,
/// remove its file, and load the agent from the file that was kept. One
/// sentence per step, each naming what it did or the exact failure.
pub async fn restore_agent_twin(
    target: &ComputeTarget,
    label: &str,
    twin: &AgentTwin,
    runner: &Runner,
) -> String {
    let mut said = Vec::new();
    said.push(
        match bootout_label(target, label, BootoutScope::System, runner).await {
            Ok((state, detail)) => format!("daemon {state} ({detail})"),
            Err(error) => format!("the daemon could not be booted out: {error}"),
        },
    );
    let daemon = format!("{LAUNCHD_DAEMONS}/{label}.plist");
    said.push(match remove(target, &daemon, true, runner).await {
        Ok(path) => format!("daemon file {path} gone"),
        Err(error) => format!("the daemon file could not be removed: {error}"),
    });
    said.push(match &twin.path {
        Some(_) => match load_label(target, label, runner).await {
            Ok((state, detail)) => format!("agent {state} ({detail})"),
            Err(error) => format!("the agent could not be loaded again: {error}"),
        },
        None => "the agent had no file, so it was not loaded again".to_string(),
    });
    said.join("; ")
}
