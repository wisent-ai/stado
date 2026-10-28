//! The environment a rendered unit exports, in Python dict insertion order:
//! the Skarbiec connection metadata and the process inputs an installed
//! service reads its backend routing out of.

use std::path::Path;

/// The unit environment, in Python dict insertion order (the plist/unit
/// renderers iterate this order byte-exactly).
pub fn build_env(kind: &str) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![("PYTHONUNBUFFERED".to_string(), "1".to_string())];
    let agent_url = crate::config::agent_skarbiec_url();
    let (skarbiec_url, skarbiec_consumer, skarbiec_token_file) = (
        crate::config::skarbiec_url(),
        crate::config::skarbiec_consumer(),
        crate::config::skarbiec_token_file(),
    );
    env.push(("WC_SKARBIEC_URL".to_string(), skarbiec_url.to_string()));
    env.push((
        "WC_SKARBIEC_CONSUMER".to_string(),
        skarbiec_consumer.to_string(),
    ));
    env.push((
        "WC_SKARBIEC_TOKEN_FILE".to_string(),
        skarbiec_token_file.to_string(),
    ));
    if kind == "host" {
        // A named agent service this host cannot resolve now is installed as
        // configured, so the unit resolves it when it reads it; read as empty
        // it was replaced by the shared Skarbiec URL.
        let installed = if crate::config::agent_skarbiec_url_absent().is_some() {
            crate::config::agent_skarbiec_url_configured()
        } else if agent_url.is_empty() {
            crate::config::skarbiec_url()
        } else {
            agent_url
        };
        env.push(("WC_AGENT_SKARBIEC_URL".to_string(), installed.to_string()));
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
    // grant. Job payloads inherit PATH: the declared search path, never the
    // installing shell's, so every host unit finds the same tools.
    let runs_local_agent = kind == "host";
    if runs_local_agent {
        env.push((
            "PATH".to_string(),
            crate::primitives::tool_path::SEARCH_PATH.to_string(),
        ));
    }
    // Unspecified installer defaults are omitted here. Renderers retain explicit
    // empty values read from existing native declarations during consolidation.
    env.retain(|(_, value)| !value.is_empty());
    env
}

/// [`build_env`] with the provider-neutral process inputs used by installed
/// services. Backend routing comes only from `STADO_CONFIG`; a config that
/// cannot be read refuses the install, since a unit installed without it ran
/// on built-in routing instead of this machine's declaration.
pub fn install_env(
    _home: &Path,
    kind: &str,
    _hf_token: &str,
) -> Result<Vec<(String, String)>, crate::deploy::DeployError> {
    let mut env = build_env(kind);
    let config = crate::config_file::config_path().map_err(|error| {
        crate::deploy::DeployError(format!(
            "the Stado config this unit is to run with cannot be read: {error}"
        ))
    })?;
    if let Some(path) = config {
        env.push((
            "STADO_CONFIG".to_string(),
            path.to_string_lossy().into_owned(),
        ));
    }
    let deployment_id = crate::config::stado_deployment_id();
    if !deployment_id.is_empty() {
        env.push(("STADO_DEPLOYMENT_ID".to_string(), deployment_id));
    }
    Ok(env)
}
