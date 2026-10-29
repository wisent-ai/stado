//! The reads an upgrade makes before it changes anything besides the source,
//! the installed program and the checks: the builder, the host's declared
//! SSH keepalive, its claim gates, and the operator's go-ahead.

use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

use super::report::State;

/// The registry host that compiles the revision: `--builder`, else this
/// machine when it shares the target's release platform, else the target.
pub(super) async fn builder(
    requested: Option<&str>,
    target: &ComputeTarget,
) -> Result<ComputeTarget, CmdError> {
    let chosen = match requested {
        Some(name) => crate::cli::canonical_host(name).await?,
        None => {
            let registry = crate::deploy::host_channel::canonical_registry()
                .await
                .map_err(CmdError::from)?;
            let hostname = crate::providers::vast::system_hostname();
            match registry.lookup_self(&hostname).map_err(CmdError::from)? {
                Some(this) if this.release_platform == target.release_platform => this.clone(),
                _ => target.clone(),
            }
        }
    };
    if chosen.release_platform != target.release_platform {
        return Err(CmdError::refused(format!(
            "--builder {} builds for {}, and {} runs {}; name a builder of {}",
            chosen.name,
            chosen.release_platform,
            target.name,
            target.release_platform,
            target.release_platform
        )));
    }
    Ok(chosen)
}

/// What the host's configuration declares about SSH keepalive, which an
/// incoming Stado requires on a host that runs a resolver; the incoming
/// program judges the whole configuration again before it installs.
pub(super) async fn declarations(target: &ComputeTarget, runner: &Runner) -> (State, String) {
    let shown = crate::cli::host::remote_config_output(
        target,
        crate::cli::host::RemoteConfigAction::Show,
        runner,
    )
    .await;
    let document: Value = match shown.map(|text| serde_json::from_str(&text)) {
        Ok(Ok(document)) => document,
        Ok(Err(error)) => {
            return (
                State::Failed,
                format!("config show did not answer JSON: {error}"),
            )
        }
        Err(error) => return (State::Failed, error.to_string()),
    };
    let mut keepalive = Vec::new();
    collect_keepalive(&document, "", &mut keepalive);
    let file = match document["file"].as_str() {
        Some(file) => file.to_string(),
        None => "no configuration file".to_string(),
    };
    let said = if keepalive.is_empty() {
        format!(
            "{file} declares no SSH keepalive; an incoming Stado refuses to replace one that \
             runs a resolver until both are declared (`stado host config set {} \
             ssh.keepalive_seconds SECONDS` and `… ssh.keepalive_count_max COUNT`)",
            target.name
        )
    } else {
        format!("{file} declares {}", keepalive.join(", "))
    };
    (
        State::Ok,
        format!("{said}; the incoming program runs `config validate` there before it installs"),
    )
}

fn collect_keepalive(value: &Value, path: &str, found: &mut Vec<String>) {
    if let Value::Object(fields) = value {
        for (key, field) in fields {
            let at = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            if key.contains("keepalive") && !field.is_object() {
                found.push(format!("{at} = {field}"));
            }
            collect_keepalive(field, &at, found);
        }
    }
}

/// The host's claim gates, disk and beacon age, as one line.
pub(super) async fn gates(host: &str, runner: &Runner) -> (State, String) {
    match crate::deploy::host_gates::read_host_gates(host, runner).await {
        Ok(gates) => (
            State::Ok,
            format!(
                "claiming {}, disk {} used, beacon {}{}",
                gates.claiming,
                match gates.used_percent {
                    Some(percent) => format!("{percent:.1}%"),
                    None => "not measured".to_string(),
                },
                match gates.age_seconds {
                    Some(age) => format!("{age} s old"),
                    None => "never published".to_string(),
                },
                if gates.blockers.is_empty() {
                    String::new()
                } else {
                    format!(", blockers: {}", gates.blockers.join(", "))
                }
            ),
        ),
        Err(error) => (State::Failed, error.to_string()),
    }
}

/// Read a line from this terminal: the host's name typed back starts the
/// upgrade.
pub(super) async fn confirmed(host: &str) -> Result<bool, CmdError> {
    use tokio::io::AsyncBufReadExt as _;
    eprint!("Someone must be at {host} to answer the dialogs above. Type {host} to start: ");
    let mut line = String::new();
    tokio::io::BufReader::new(tokio::io::stdin())
        .read_line(&mut line)
        .await
        .map_err(|error| CmdError::click(format!("this terminal could not be read: {error}")))?;
    Ok(line.trim() == host)
}
