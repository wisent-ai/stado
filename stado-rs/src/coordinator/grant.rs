//! The scoped remote workload grant the tick projects into agent templates.

use std::collections::BTreeMap;

use base64::Engine as _;

use crate::config;

/// Internal scheduler-map key for the Azure agent grant. This deliberately is
/// not an environment-variable name and is never eligible for startup-script
/// substitution; the Azure provider consumes it only as protected settings.
pub(crate) const AZURE_AGENT_PROTECTED_GRANT: &str = "stado.protected-settings.azure-agent-grant";

/// Base64-encoded scoped workload grant projected only into non-Azure agent
/// templates. Azure receives the same grant through protected settings.
pub(crate) const AGENT_WORKLOAD_GRANT_B64: &str = "STADO_AGENT_SKARBIEC_GRANT_B64";

/// Validate the dedicated remote workload grant and return its opaque token.
///
/// The grant exposes only the configured workload-secret items. Azure receives
/// it through encrypted protected settings; other cloud startup templates
/// materialize the same scoped token into a root-only tmpfs file.
pub(crate) async fn agent_workload_grant() -> Result<Option<String>, crate::skarbiec::SkarbiecError>
{
    use crate::skarbiec::SkarbiecError;

    let remote_agents = config::wc_providers().iter().any(|name| {
        crate::capabilities::execution_adapter(name)
            .is_some_and(|adapter| adapter != crate::capabilities::ExecutionAdapter::Local)
    });
    if !remote_agents {
        return Ok(None);
    }
    let url = config::agent_skarbiec_url();
    if url.is_empty() {
        return Err(SkarbiecError::Deployment(
            "WC_AGENT_SKARBIEC_URL is required for remote workload agents; set it to an HTTPS \
             Skarbiec endpoint reachable from every agent VM"
                .to_string(),
        ));
    }
    if !url.starts_with("https://") {
        return Err(SkarbiecError::Deployment(format!(
            "WC_AGENT_SKARBIEC_URL={url:?} is not HTTPS; a remote workload grant must never \
             cross plaintext HTTP"
        )));
    }
    let consumer = config::agent_skarbiec_consumer();
    // A naming contract on an identity Stado itself provisions, enforced here at
    // the boundary where deployment configuration is read, and it fails closed.
    //
    // It is a requirement, not an inference: nothing downstream derives
    // behaviour from the suffix, and the exclusion list below is the proof that
    // the suffix alone never identified a remote workload agent —
    // `stado-local-agent` and `stado-azure-agent` both end in `-agent` and are
    // both rejected. What it protects is real: this consumer's bearer is
    // projected into every agent VM's startup template (`AGENT_WORKLOAD_GRANT_B64`,
    // `AZURE_AGENT_PROTECTED_GRANT`), so accepting a broad or control-plane
    // identity here would ship that identity's grant to every remote host.
    //
    // The Skarbiec read path no longer depends on this check. Caching and
    // erasure are decided by the `GrantMode` each construction site declares, so
    // a consumer renamed in violation of this contract is refused here rather
    // than silently changing how its bearer is handled.
    if consumer.is_empty()
        || !consumer.ends_with("-agent")
        || matches!(
            consumer,
            "stado-control-plane" | "stado-local-agent" | "stado-azure-agent"
        )
    {
        return Err(SkarbiecError::Deployment(format!(
            "WC_AGENT_SKARBIEC_CONSUMER={consumer:?} must be a scoped remote workload \
             identity ending in -agent and distinct from control-plane/legacy identities"
        )));
    }
    let token_file = config::agent_skarbiec_token_file();
    if token_file.is_empty() {
        return Err(SkarbiecError::Deployment(
            "WC_AGENT_SKARBIEC_TOKEN_FILE is required; use an owner-only workload grant"
                .to_string(),
        ));
    }
    let agent_token = crate::skarbiec::read_grant(token_file)?;
    // The workload grant is materialized into the platform's handoff directory
    // on the agent VMs and is an owner-only provisioned file on the control
    // plane, so its placement is what states the mode.
    let agent_vault = crate::skarbiec::Client::new(
        url,
        consumer,
        token_file,
        crate::skarbiec::GrantMode::for_grant_file(token_file),
    )?;
    let mut visible: Vec<String> = agent_vault
        .list_items()
        .await?
        .into_iter()
        .map(|item| item.id)
        .collect();
    visible.sort();
    let mut expected = config::agent_skarbiec_items().to_vec();
    expected.sort();
    expected.dedup();
    if expected
        .iter()
        .any(|item| matches!(item.as_str(), "stado-aws" | "stado-azure" | "stado-gcp"))
    {
        return Err(SkarbiecError::Deployment(
            "agent.skarbiec.items must not contain cloud-provider credential items".to_string(),
        ));
    }
    for reference in config::agent_skarbiec_secret_fields() {
        let Some((item, field)) = reference.split_once('#') else {
            return Err(SkarbiecError::Deployment(format!(
                "agent.skarbiec.secret_fields entry {reference:?} must be item#field"
            )));
        };
        if item.is_empty()
            || field.is_empty()
            || !expected.iter().any(|configured| configured == item)
        {
            return Err(SkarbiecError::Deployment(format!(
                "agent.skarbiec.secret_fields entry {reference:?} is not covered by agent.skarbiec.items"
            )));
        }
    }
    if visible != expected {
        return Err(SkarbiecError::Deployment(format!(
            "consumer {consumer:?} can list {visible:?}; the remote workload grant must expose \
             exactly the configured workload-secret items"
        )));
    }
    Ok(Some(agent_token))
}

/// Resolve the dedicated workload grant needed by agent dispatch.
///
/// Azure consumes the raw token only through protected settings. Other remote
/// providers receive a base64 projection for root-only tmpfs materialization;
/// the renderer never logs the rendered script or any secret value.
pub(crate) async fn secrets_from_skarbiec(
) -> Result<BTreeMap<String, String>, crate::skarbiec::SkarbiecError> {
    let mut secrets = BTreeMap::new();
    if let Some(agent_token) = agent_workload_grant().await? {
        let mut azure = false;
        let mut inline = false;
        for provider in config::wc_providers() {
            match crate::capabilities::execution_adapter(provider) {
                Some(crate::capabilities::ExecutionAdapter::Azure) => azure = true,
                Some(crate::capabilities::ExecutionAdapter::Local) | None => {}
                Some(_) => inline = true,
            }
        }
        if azure {
            secrets.insert(AZURE_AGENT_PROTECTED_GRANT.to_string(), agent_token.clone());
        }
        if inline {
            secrets.insert(
                AGENT_WORKLOAD_GRANT_B64.to_string(),
                base64::engine::general_purpose::STANDARD.encode(agent_token.as_bytes()),
            );
        }
    }
    Ok(secrets)
}
