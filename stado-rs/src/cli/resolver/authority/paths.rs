use crate::service_resolution::ResolvedService;
use crate::targets;

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
