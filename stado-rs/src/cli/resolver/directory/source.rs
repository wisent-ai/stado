use std::process::Stdio;
use std::sync::Arc;

use serde_json::Value;

use crate::service_resolution;
use crate::targets::{self, RegistryStore};

use crate::cli::resolver::authority::drop_stale_ssh_sockets;
use crate::cli::resolver::authority::paths::target_ssh_paths;
use crate::cli::resolver::authority::refusal::refuse_authority;
use crate::cli::resolver::authority::ssh_command;
use crate::cli::resolver::directory::read_local_snapshot;
use crate::cli::resolver::directory::validate_snapshot;
use crate::cli::resolver::directory::SnapshotPayload;
use crate::cli::resolver::directory::AUTHORITY_FETCH_TIMEOUT;
use crate::cli::resolver::directory::SNAPSHOT_LIMIT;

#[derive(Clone)]
pub(crate) enum SnapshotSource {
    Local(Arc<RegistryStore>),
    Authority {
        /// Registry name of the authority target, carried so a failed read
        /// can name the host that is actually silent.
        target: String,
        ssh: Vec<targets::SshConnectionPath>,
        command: String,
    },
}

impl SnapshotSource {
    /// The host a failure of this source is evidence about.
    pub(crate) fn subject_host(&self, local_target: &str) -> String {
        match self {
            Self::Local(_) => local_target.to_string(),
            Self::Authority { target, .. } => target.clone(),
        }
    }

    /// `reader` is the refusal vocabulary's word for who is reading:
    /// [`host_silence::READER_RESOLVER`] for the serving loop and its
    /// background refresh, [`host_silence::READER_CLI`] for a one-shot
    /// command.
    pub(crate) async fn fetch(&self, reader: &str) -> Result<(Value, String, u64), String> {
        match self {
            Self::Local(store) => read_local_snapshot(store).await,
            // A control master outlives the process that opened it by
            // `ControlPersist`, and one whose connection has already died
            // answers nothing while looking perfectly alive. Drop its socket
            // after the first failed authority read and retry once in this
            // invocation. The old implementation left recovery to a future
            // call, so one-shot commands failed while printing that they had
            // already repaired the cause.
            Self::Authority {
                target,
                ssh,
                command,
            } => {
                let first = Self::fetch_authority_paths(target, ssh, command, reader).await;
                let first_error = match first {
                    Ok(snapshot) => return Ok(snapshot),
                    Err(error) => error,
                };
                drop_stale_ssh_sockets();
                Self::fetch_authority_paths(target, ssh, command, reader)
                    .await
                    .map_err(|retry_error| {
                        format!(
                            "{first_error}; second authority read with stale SSH control sockets removed failed: \
                             {retry_error}"
                        )
                    })
            }
        }
    }

    async fn fetch_authority_paths(
        target: &str,
        paths: &[targets::SshConnectionPath],
        command: &str,
        reader: &str,
    ) -> Result<(Value, String, u64), String> {
        if paths.is_empty() {
            return Err("registry authority has no SSH connection path".to_string());
        }
        let mut failures = Vec::new();
        for path in paths {
            match Self::fetch_authority(target, &path.destination, command, reader).await {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) => failures.push(format!("{}: {error}", path.name)),
            }
        }
        Err(format!(
            "no registry authority SSH connection path answered ({})",
            failures.join("; ")
        ))
    }

    async fn fetch_authority(
        target: &str,
        ssh: &str,
        command: &str,
        reader: &str,
    ) -> Result<(Value, String, u64), String> {
        let remote_command = format!("{} resolver snapshot", crate::deploy::shlex_quote(command));
        let output = match tokio::time::timeout(
            AUTHORITY_FETCH_TIMEOUT,
            ssh_command("ControlMaster=no")
                .arg(ssh)
                .arg(remote_command)
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                let sentence = format!("registry authority SSH failed: {error}");
                refuse_authority(target, reader, &sentence).await;
                return Err(sentence);
            }
            Err(_) => {
                let sentence = format!(
                    "registry authority SSH timed out after {}s",
                    AUTHORITY_FETCH_TIMEOUT.as_secs()
                );
                refuse_authority(target, reader, &sentence).await;
                return Err(sentence);
            }
        };
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            let detail = detail.trim();
            let sentence = if detail.is_empty() {
                format!("registry authority exited with {}", output.status)
            } else {
                format!(
                    "registry authority exited with {}: {}",
                    output.status,
                    detail.chars().take(4096).collect::<String>()
                )
            };
            // Only the two transport branches publish. An authority that
            // answers with an oversized or unparseable snapshot is reachable
            // and wrong, which is a different finding from a silent host and
            // must not be counted as one.
            refuse_authority(target, reader, &sentence).await;
            return Err(sentence);
        }
        if output.stdout.len() > SNAPSHOT_LIMIT {
            return Err("registry authority snapshot exceeds 1 MiB".to_string());
        }
        let payload: SnapshotPayload = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("invalid registry authority response: {error}"))?;
        validate_snapshot(payload)
    }
}

fn parsed_registry(document: &Value) -> Result<targets::Registry, String> {
    targets::load_registry_from_str(
        &serde_json::to_string(document).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn current_target(document: &Value) -> Result<String, String> {
    let hostname = crate::providers::vast::system_hostname();
    parsed_registry(document)?
        .lookup_self(&hostname)
        .map_err(|error| error.to_string())?
        .map(|target| target.name.clone())
        .ok_or_else(|| format!("resolver host {hostname:?} has no registry target identity"))
}

pub(crate) fn snapshot_source(
    local_store: Option<Arc<RegistryStore>>,
    document: &Value,
    local_target: &str,
) -> Result<SnapshotSource, String> {
    let directory = service_resolution::directory(document)?
        .ok_or_else(|| "registry.service_directory is required".to_string())?;
    if directory.authority.target == local_target {
        return local_store
            .map(SnapshotSource::Local)
            .ok_or_else(|| "local registry authority backend is unavailable".to_string());
    }
    let registry = parsed_registry(document)?;
    let target = registry
        .lookup(&directory.authority.target)
        .ok_or_else(|| "registry authority target disappeared".to_string())?;
    let ssh = target_ssh_paths(target);
    if ssh.is_empty() {
        return Err("registry authority has no SSH connection path".to_string());
    }
    Ok(SnapshotSource::Authority {
        target: target.name.clone(),
        ssh,
        command: directory.authority.command,
    })
}
