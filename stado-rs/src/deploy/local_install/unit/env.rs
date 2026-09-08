//! The environment a rendered unit exports, in Python dict insertion order:
//! the interpreter the agent's Python probes and job payloads run under, the
//! Skarbiec connection metadata, and the process inputs an installed service
//! reads its backend routing out of.

use std::path::Path;

use super::exec::local_control_plane_configured;

/// The python.org 3.12 framework interpreter the fleet's mac minis install
/// the job environment (wisent, transformers, ...) into.
pub const FRAMEWORK_PYTHON: &str =
    "/Library/Frameworks/Python.framework/Versions/3.12/bin/python3.12";

/// The WC_PYTHON value baked into agent units: the Rust agent's Python
/// probes (smoketest, CUDA probe, fleet flush) and the job payloads still
/// run as Python, so the unit must point at the interpreter that has the
/// job environment installed. Operator override via $WC_PYTHON first, then
/// [`FRAMEWORK_PYTHON`] when present, else plain `python3`.
pub fn default_wc_python() -> String {
    if let Ok(value) = std::env::var("WC_PYTHON") {
        if !value.trim().is_empty() {
            return value;
        }
    }
    if Path::new(FRAMEWORK_PYTHON).is_file() {
        return FRAMEWORK_PYTHON.to_string();
    }
    "python3".to_string()
}

/// Explicit inputs used by the provider-neutral unit renderer.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvInputs<'a> {
    pub wc_python: &'a str,
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
    if kind == "agent" {
        env.push((
            "WC_AGENT_SKARBIEC_URL".to_string(),
            skarbiec_url.to_string(),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_CONSUMER".to_string(),
            skarbiec_consumer.to_string(),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_TOKEN_FILE".to_string(),
            skarbiec_token_file.to_string(),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_ITEMS".to_string(),
            crate::config::agent_skarbiec_items().join(","),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_SECRET_FIELDS".to_string(),
            crate::config::agent_skarbiec_secret_fields().join(","),
        ));
    }
    // The standalone agent and the outage-safe local control plane both
    // execute Python probes and job payloads. Preserve the operator PATH so
    // child jobs see the same toolchain as an interactive Stado invocation.
    let runs_local_agent =
        kind == "agent" || (kind == "coordinator" && local_control_plane_configured());
    if runs_local_agent {
        if !inputs.wc_python.is_empty() {
            env.push(("WC_PYTHON".to_string(), inputs.wc_python.to_string()));
        }
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
    env
}

/// [`build_env`] with the provider-neutral process inputs used by installed
/// services. Backend routing comes only from `STADO_CONFIG`.
pub fn install_env(
    _home: &Path,
    kind: &str,
    _hf_token: &str,
    wc_python: &str,
) -> Vec<(String, String)> {
    let path = std::env::var("PATH").ok();
    let inputs = EnvInputs {
        wc_python,
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
