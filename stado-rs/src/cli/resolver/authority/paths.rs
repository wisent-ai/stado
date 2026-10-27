use crate::service_resolution::ResolvedService;
use crate::targets;

pub(crate) fn target_ssh_paths(target: &targets::ComputeTarget) -> Vec<targets::SshConnectionPath> {
    reachable(
        target
            .ssh_connections()
            .map(|(name, destination)| targets::SshConnectionPath {
                name: name.to_string(),
                destination: destination.to_string(),
            })
            .collect(),
    )
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
    reachable(paths)
}

/// `paths` without those the tailnet itself states it cannot carry, each
/// left out named with the cause on stderr. A path through a node whose
/// Tailscale key expired only times out; raced beside a working path it
/// spends the read's whole deadline and hides the cause. When every path is
/// such a one they are all kept, so the connection fails naming the expiry.
fn reachable(paths: Vec<targets::SshConnectionPath>) -> Vec<targets::SshConnectionPath> {
    let mut live = Vec::with_capacity(paths.len());
    let mut dead = Vec::new();
    for path in paths {
        match crate::remote::tailnet::destination_unreachable_because(&path.destination) {
            Some(cause) => dead.push((path, cause)),
            None => live.push(path),
        }
    }
    if live.is_empty() {
        return dead.into_iter().map(|(path, _)| path).collect();
    }
    for (path, cause) in &dead {
        eprintln!(
            "stado: SSH path {} ({}) is left out: the tailnet carries no traffic to it, {cause}",
            path.name, path.destination
        );
    }
    live
}
