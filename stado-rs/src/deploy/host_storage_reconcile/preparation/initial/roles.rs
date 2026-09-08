use super::*;

fn command_tokens(command: &str) -> Vec<&str> {
    command
        .split_ascii_whitespace()
        .map(|token| token.trim_matches(|ch| matches!(ch, '{' | '}' | '[' | ']' | ';' | ',' | '"')))
        .filter(|token| !token.is_empty())
        .collect()
}

pub(in crate::deploy::host_storage_reconcile) fn executable_name(token: &str) -> &str {
    token.rsplit('/').next().unwrap_or(token)
}

pub(in crate::deploy::host_storage_reconcile) fn storage_route_key(key: &str) -> bool {
    matches!(
        key,
        "STADO_CONFIG"
            | "WC_STORAGE_BACKEND"
            | "WC_LOCAL_STORAGE_PATH"
            | "WC_BACKUP_STORAGE_BACKEND"
            | "WC_BACKUP_LOCAL_STORAGE_PATH"
            | "WC_STADO_STORAGE_URL"
            | "WC_STADO_STORAGE_NAMESPACE"
            | "WC_STADO_STORAGE_TOKEN_FILE"
    )
}

pub(in crate::deploy::host_storage_reconcile) fn service_role(
    label: &str,
    command: &str,
) -> &'static str {
    const OBJECT_API_LABEL: &str = "com.wisent.always-on.stado-object-api";
    if label == OBJECT_API_LABEL {
        return "object-api";
    }
    let tokens = command_tokens(command);
    if tokens
        .iter()
        .any(|token| executable_name(token) == "Runner.Listener")
    {
        return "runner";
    }
    let executable = tokens
        .iter()
        .position(|token| executable_name(token) == "stado");
    if executable.is_some_and(|index| {
        tokens.get(index + 1).copied() == Some("release")
            && tokens.get(index + 2).copied() == Some("agent")
    }) {
        return "release-agent";
    }
    if let Some(index) = executable {
        return match tokens.get(index + 1).copied() {
            Some("resolver") => "transport",
            Some("coordinator" | "local-control-plane" | "cloud-control-plane") => "coordinator",
            Some("agent") => "agent",
            Some("disk-cleanup") => "disk-cleanup",
            _ => "writer",
        };
    }
    match tokens
        .first()
        .map(|token| executable_name(token))
        .unwrap_or_default()
    {
        "caddy" | "cloudflared" | "tailscaled" | "skarbiec" | "skarbiec-control-plane" | "ssh" => {
            "transport"
        }
        "stado-fix" => "agent",
        _ => "other",
    }
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
    match role {
        "runner" => 0,
        "current-runner" => 1,
        "release-agent" => 2,
        "coordinator" => 3,
        "agent" | "disk-cleanup" => 4,
        "object-api" => u8::MAX,
        _ => 5,
    }
}
