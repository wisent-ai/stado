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
    // A connection that names no login lands on whatever user the transport
    // picks, which is not the binding's user as far as anyone stated.
    target.has_ssh_connection()
        && target.ssh_connections().all(|(_, destination)| {
            destination
                .split_once('@')
                .is_some_and(|(login, _)| login == declared)
        })
}

/// Is this registry target the machine we are running on?
///
/// Matched on the short hostname and the declared hostnames, because a registry name
/// is an operator label ("operator-host") and need not equal what the OS reports.
pub(in crate::cli::identity) fn is_local_target(target: &ComputeTarget) -> bool {
    let host = crate::providers::vast::system_hostname().to_lowercase();
    if host.is_empty() {
        return false;
    }
    // Compare the leading label only. The kernel names "Operator-MacBook-Pro"
    // while the registry records the mDNS form "operator-host.local", and
    // an exact match fails on that suffix unannounced -- reporting the local machine as
    // unverifiable while standing on it.
    let label = |value: &str| {
        let value = value.to_lowercase();
        match value.split_once('.') {
            Some((first, _)) => first.to_string(),
            None => value,
        }
    };
    let host = label(&host);
    label(&target.name) == host || target.hostnames.iter().any(|name| label(name) == host)
}

/// The Apple accounts the current user is signed into on this machine, or
/// none when they cannot be read; why not is said on stderr, so an unknown
/// answer carries its cause.
pub(in crate::cli::identity) fn local_apple_accounts() -> Option<Vec<String>> {
    let output = match crate::wait::output(
        std::process::Command::new("defaults").args(["read", "MobileMeAccounts"]),
    ) {
        Ok(output) => output,
        Err(error) => {
            eprintln!(
                "this machine's Apple accounts cannot be read: defaults did not run: {error}"
            );
            return None;
        }
    };
    if !output.status.success() {
        eprintln!(
            "this machine's Apple accounts cannot be read: defaults read MobileMeAccounts ended {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    let found = account_ids(&String::from_utf8_lossy(&output.stdout));
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}
