use std::process::Stdio;
use std::time::Duration;

use crate::service_resolution::ResolvedService;
use crate::targets;

use crate::cli::resolver::authority::drop_stale_ssh_sockets;
use crate::cli::resolver::authority::ssh_proxy_command;

pub(crate) fn target_ssh_paths(target: &targets::ComputeTarget) -> Vec<targets::SshConnectionPath> {
    target
        .ssh_connections()
        .map(|(name, destination)| targets::SshConnectionPath {
            name: name.to_string(),
            destination: destination.to_string(),
        })
        .collect()
}

pub(crate) fn resolved_ssh_paths(resolved: &ResolvedService) -> Vec<targets::SshConnectionPath> {
    let mut paths =
        Vec::with_capacity(usize::from(resolved.ssh.is_some()) + resolved.ssh_fallbacks.len());
    if let Some(destination) = &resolved.ssh {
        paths.push(targets::SshConnectionPath {
            name: targets::PRIMARY_SSH_CONNECTION.to_string(),
            destination: destination.clone(),
        });
    }
    paths.extend(resolved.ssh_fallbacks.iter().cloned());
    paths
}

pub(crate) async fn select_resolver_ssh_path(
    paths: &[targets::SshConnectionPath],
) -> Result<&targets::SshConnectionPath, String> {
    let Some(first) = paths.first() else {
        return Err("active host has no registry SSH connection path".to_string());
    };
    if paths.len() == 1 {
        return Ok(first);
    }

    let mut failures = Vec::new();
    for path in paths {
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            ssh_proxy_command()
                .arg(&path.destination)
                .arg("true")
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output(),
        )
        .await;
        match result {
            Ok(Ok(output)) if output.status.success() => return Ok(path),
            Ok(Ok(output)) => {
                let detail = String::from_utf8_lossy(&output.stderr);
                let detail = detail.trim();
                failures.push(format!(
                    "{}: {}",
                    path.name,
                    if detail.is_empty() {
                        output.status.to_string()
                    } else {
                        detail.chars().take(512).collect()
                    }
                ));
            }
            Ok(Err(error)) => failures.push(format!("{}: {error}", path.name)),
            Err(_) => failures.push(format!("{}: timed out after 20s", path.name)),
        }
    }
    // Both call sites of the cleanup below ran at startup, which was enough
    // while this service restarted every few minutes: 176 launchd runs on
    // 2026-09-02, each one arriving at a clean slate. With the descriptor
    // ceiling raised the process now stays up, and a master that dies inside
    // one lifetime leaves its socket file behind for the rest of it. ssh then
    // says `ControlSocket ... already exists, disabling multiplexing` and
    // opens a private connection per request until the authority's sshd
    // resets them, which reads from the outside as the object store being
    // unreachable. Unlinking costs nothing - live sessions keep their
    // descriptors and the next attempt opens a fresh master - so a path that
    // answered nothing gets its socket dropped here rather than at the next
    // restart that no longer comes.
    drop_stale_ssh_sockets();
    Err(format!(
        "no registry SSH connection path answered ({})",
        failures.join("; ")
    ))
}
