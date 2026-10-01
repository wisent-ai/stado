//! The environment a rendered unit exports, in Python dict insertion order:
//! the Skarbiec connection metadata and the process inputs an installed
//! service reads its backend routing out of.

use std::path::Path;

use super::exec::local_control_plane_configured;

/// Explicit inputs used by the provider-neutral unit renderer.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvInputs<'a> {
    pub path: Option<&'a str>,
}

/// The unit environment, in Python dict insertion order (the plist/unit
/// renderers iterate this order byte-exactly).
pub fn build_env(kind: &str, inputs: &EnvInputs) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![("PYTHONUNBUFFERED".to_string(), "1".to_string())];
    let agent_url = crate::config::agent_skarbiec_url();
    let (skarbiec_url, skarbiec_consumer, skarbiec_token_file) = if kind == "agent" {
        (
            if agent_url.is_empty() {
                crate::config::skarbiec_url()
            } else {
                agent_url
            },
            crate::config::agent_skarbiec_consumer(),
            crate::config::agent_skarbiec_token_file(),
        )
    } else {
        (
            crate::config::skarbiec_url(),
            crate::config::skarbiec_consumer(),
            crate::config::skarbiec_token_file(),
        )
    };
    env.push(("WC_SKARBIEC_URL".to_string(), skarbiec_url.to_string()));
    env.push((
        "WC_SKARBIEC_CONSUMER".to_string(),
        skarbiec_consumer.to_string(),
    ));
    env.push((
        "WC_SKARBIEC_TOKEN_FILE".to_string(),
        skarbiec_token_file.to_string(),
    ));
    if matches!(kind, "agent" | "host") {
        env.push((
            "WC_AGENT_SKARBIEC_URL".to_string(),
            if agent_url.is_empty() {
                crate::config::skarbiec_url()
            } else {
                agent_url
            }
            .to_string(),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_CONSUMER".to_string(),
            crate::config::agent_skarbiec_consumer().to_string(),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_TOKEN_FILE".to_string(),
            crate::config::agent_skarbiec_token_file().to_string(),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_ROLES".to_string(),
            crate::config::agent_skarbiec_roles().join(","),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_SECRET_FIELDS".to_string(),
            crate::config::agent_skarbiec_secret_fields().join(","),
        ));
    }
    // The host carries the workload grant separately from its control-plane
    // grant. Job payloads inherit PATH; their runtime is the workload's own.
    let runs_local_agent = matches!(kind, "agent" | "host")
        || (kind == "coordinator" && local_control_plane_configured());
    if runs_local_agent {
        let path = inputs
            .path
            .unwrap_or("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
        env.push(("PATH".to_string(), path.to_string()));
    }
    // Failure-fixer and watchdog resolve credentials and backend routing
    // through Stado config and Skarbiec. Only PATH is inherited here.
    if kind == "failure-fixer" || kind == "watchdog" {
        let path = inputs
            .path
            .unwrap_or("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
        env.push(("PATH".to_string(), path.to_string()));
    }
    // Unspecified installer defaults are omitted here. Renderers retain explicit
    // empty values read from existing native declarations during consolidation.
    env.retain(|(_, value)| !value.is_empty());
    env
}

/// [`build_env`] with the provider-neutral process inputs used by installed
/// services. Backend routing comes only from `STADO_CONFIG`.
pub fn install_env(_home: &Path, kind: &str, _hf_token: &str) -> Vec<(String, String)> {
    let path = std::env::var("PATH").ok();
    let inputs = EnvInputs {
        path: path.as_deref(),
    };
    let mut env = build_env(kind, &inputs);
    if let Ok(Some(path)) = crate::config_file::config_path() {
        env.push((
            "STADO_CONFIG".to_string(),
            path.to_string_lossy().into_owned(),
        ));
    }
    let deployment_id = crate::config::stado_deployment_id();
    if !deployment_id.is_empty() {
        env.push(("STADO_DEPLOYMENT_ID".to_string(), deployment_id));
    }
    env
}
