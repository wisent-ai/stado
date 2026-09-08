//! The machine this command is running on, and the channel's own login user.

use super::accounts::account_ids;
use crate::targets::{ComputeTarget, IdentityBinding};

/// Does the approved channel land on the very user this binding names?
///
/// Every declared connection must log in as the binding's user. A second choice
/// that lands on another account would make the observation depend on which
/// network happened to answer first.
pub(in crate::cli::identity) fn probes_own_user(
    target: &ComputeTarget,
    binding: &IdentityBinding,
) -> bool {
    let Some(declared) = binding.user.as_deref() else {
        return true;
    };
    target.has_ssh_connection()
        && target.ssh_connections().all(|(_, destination)| {
            destination
                .split_once('@')
                .map(|(login, _)| login == declared)
                .unwrap_or(false)
        })
}

/// Is this registry target the machine we are running on?
///
/// Matched on the short hostname and the declared hostnames, because a registry name
/// is an operator label ("operator-host") and need not equal what the OS reports.
pub(in crate::cli::identity) fn is_local_target(target: &ComputeTarget) -> bool {
    let Ok(output) = std::process::Command::new("hostname").arg("-s").output() else {
        return false;
    };
    let host = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();
    if host.is_empty() {
        return false;
    }
    // Compare the leading label only. `hostname -s` gives "Lukaszs-MacBook-Pro-5485"
    // while the registry records the mDNS form "operator-host.local", and
    // an exact match fails on that suffix unannounced -- reporting the local machine as
    // unverifiable while standing on it.
    let label = |value: &str| {
        value
            .to_lowercase()
            .split('.')
            .next()
            .unwrap_or("")
            .to_string()
    };
    let host = label(&host);
    label(&target.name) == host || target.hostnames.iter().any(|name| label(name) == host)
}

/// The Apple accounts the current user is signed into on this machine.
pub(in crate::cli::identity) fn local_apple_accounts() -> Option<Vec<String>> {
    let output = std::process::Command::new("defaults")
        .args(["read", "MobileMeAccounts"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let found = account_ids(&String::from_utf8_lossy(&output.stdout));
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}
