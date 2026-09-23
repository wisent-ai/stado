use std::sync::Arc;

use serde_json::Value;

use crate::service_resolution;
use crate::targets::{self, RegistryStore};

use crate::cli::resolver::authority::execute::execute;
use crate::cli::resolver::authority::paths::target_ssh_paths;
use crate::cli::resolver::authority::refusal::refuse_authority;
use crate::cli::resolver::directory::read_local_snapshot;
use crate::cli::resolver::directory::validate_snapshot;
use crate::cli::resolver::directory::SnapshotPayload;

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
            // Each authority read owns a native connection independent of adapter traffic.
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
                Self::fetch_authority_paths(target, ssh, command, reader)
                    .await
                    .map_err(|retry_error| {
                        format!(
                            "{first_error}; second authority read on a fresh SSH session failed: \
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
        let output = match execute(ssh, &remote_command, SNAPSHOT_LIMIT).await {
            Ok(output) => output,
            Err(error) => {
                let sentence = format!("registry authority SSH failed: {error:#}");
                refuse_authority(target, reader, &sentence).await;
                return Err(sentence);
            }
        };
        if output.stdout_exceeded {
            return Err("registry authority snapshot exceeds 1 MiB".to_string());
        }
        if output.exit_status != Some(0) {
            let status = output.exit_status.map(|code| code.to_string())
                .unwrap_or_else(|| "no exit status".to_string());
            let detail = String::from_utf8_lossy(&output.stderr);
            let detail = detail.trim();
            let sentence = if detail.is_empty() {
                format!("registry authority exited with {status}")
            } else {
                format!(
                    "registry authority exited with {}: {}",
                    status,
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
