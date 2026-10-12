//! Which part a unit plays in a storage-root reconcile, read from what the
//! unit runs and declares.
//!
//! A unit is a storage writer when its environment names the storage route,
//! whatever program it runs; one that names no route is retained untouched.
//! A list of transport programs (`caddy`, `cloudflared`, `tailscaled`,
//! `skarbiec`, `ssh`) used to be retained by name even when their unit named
//! the route, so a program that read the store under a listed name was never
//! fenced and one under an unlisted name was guessed at.

use super::*;

/// The environment keys that route a process to the store.
const STORAGE_ROUTE_KEYS: &[&str] = &[
    "STADO_CONFIG",
    "WC_STORAGE_BACKEND",
    "WC_LOCAL_STORAGE_PATH",
    "WC_BACKUP_STORAGE_BACKEND",
    "WC_BACKUP_LOCAL_STORAGE_PATH",
    "WC_STADO_STORAGE_URL",
    "WC_STADO_STORAGE_NAMESPACE",
    "WC_STADO_STORAGE_TOKEN_FILE",
];

/// The order units are stopped in: runners first, so no job is claimed
/// against a store that is moving, the object API last.
const STOP_ORDER: &[&str] = &["runner", "current-runner", "disk-cleanup"];

fn command_tokens(command: &str) -> Vec<&str> {
    command
        .split_ascii_whitespace()
        .map(|token| token.trim_matches(|ch| matches!(ch, '{' | '}' | '[' | ']' | ';' | ',' | '"')))
        .filter(|token| !token.is_empty())
        .collect()
}

pub(in crate::deploy::host_storage_reconcile) fn executable_name(token: &str) -> &str {
    match token.rsplit('/').next() {
        Some(name) => name,
        None => token,
    }
}

pub(in crate::deploy::host_storage_reconcile) fn storage_route_key(key: &str) -> bool {
    STORAGE_ROUTE_KEYS.contains(&key)
}

pub(in crate::deploy::host_storage_reconcile) fn service_role(
    label: &str,
    command: &str,
) -> &'static str {
    // The compiled catalog is validated before anything runs, so an error
    // here only means no unit is the host process.
    let line = command_tokens(command).join(" ");
    if crate::deploy::service_catalog::runs_host_process(label, &line) == Ok(true) {
        return "object-api";
    }
    let tokens = command_tokens(command);
    if tokens
        .iter()
        .any(|token| executable_name(token) == "Runner.Listener")
    {
        return "runner";
    }
    if tokens.iter().any(|token| executable_name(token) == "stado") {
        // Every resident Stado role runs in the host process above; any other
        // Stado unit is a writer of the store.
        return if tokens.contains(&"disk-cleanup") {
            "disk-cleanup"
        } else {
            "writer"
        };
    }
    "other"
}

pub(in crate::deploy::host_storage_reconcile) fn managed_from_unit(
    target: &crate::targets::ComputeTarget,
    label: &str,
    path: &str,
    kind: &str,
) -> service::ManagedService {
    if kind == service::KIND_SYSTEMD {
        service::systemd_service(
            &target.name,
            label,
            path,
            service::SOURCE_PRODUCT,
            "storage-root-reconcile",
        )
    } else {
        service::launchd_service(
            &target.name,
            label,
            path,
            service::SOURCE_PRODUCT,
            "storage-root-reconcile",
        )
    }
}

fn exact_identity_component(value: &str, identity: &str) -> bool {
    value
        .split(['.', '/', '\\'])
        .any(|component| component == identity)
}

pub(in crate::deploy::host_storage_reconcile) fn current_runner_candidate(
    candidate: &ServiceCandidate,
    command: &str,
    current_runner: &str,
) -> bool {
    exact_identity_component(candidate.declared.unit_id(), current_runner)
        || exact_identity_component(&candidate.declared.path, current_runner)
        || command_tokens(command).contains(&current_runner)
}

pub(in crate::deploy::host_storage_reconcile) fn command_u16_option(
    command: &str,
    option: &str,
) -> Option<u16> {
    let tokens = command_tokens(command);
    tokens
        .windows(2)
        .find(|pair| pair[0] == option)
        .and_then(|pair| pair[1].parse().ok())
}

pub(in crate::deploy::host_storage_reconcile) fn stop_priority(role: &str) -> u8 {
    if role == "object-api" {
        return u8::MAX;
    }
    let place = match STOP_ORDER.iter().position(|ordered| *ordered == role) {
        Some(index) => index,
        None => STOP_ORDER.len(),
    };
    match u8::try_from(place) {
        Ok(priority) => priority,
        Err(_) => u8::MAX,
    }
}
