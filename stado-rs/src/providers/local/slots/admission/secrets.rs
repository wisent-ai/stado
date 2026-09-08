//! Workload secrets: the one Skarbiec client both the pre-claim probe and
//! the read go through, the metadata-only question of whether THIS host can
//! resolve what a job declares, and the resolution itself.

use super::*;

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// The Skarbiec client this agent resolves workload secrets through.
///
/// Factored out so the pre-claim probe and the resolution itself cannot
/// disagree about which broker, consumer and grant are in play: a check that
/// asks a different endpoint than the read would use proves nothing.
fn agent_secret_client() -> Result<crate::skarbiec::Client, StorageError> {
    let agent_token_file = crate::config::agent_skarbiec_token_file();
    if Path::new(agent_token_file).is_file() {
        let configured_url = crate::config::agent_skarbiec_url();
        let url = if configured_url.trim().is_empty() {
            crate::config::skarbiec_url()
        } else {
            configured_url
        };
        crate::skarbiec::Client::direct(
            url,
            crate::config::agent_skarbiec_consumer(),
            agent_token_file,
            crate::skarbiec::GrantMode::for_grant_file(agent_token_file),
        )
    } else if crate::config::skarbiec_consumer() == crate::config::agent_skarbiec_consumer()
        && crate::config::skarbiec_token_file() == agent_token_file
        && crate::skarbiec::GrantMode::for_grant_file(agent_token_file)
            == crate::skarbiec::GrantMode::TransientHandoff
    {
        crate::skarbiec::Client::configured()
    } else {
        return Err(StorageError::Other(
            "workload secrets require a dedicated agent Skarbiec grant".to_string(),
        ));
    }
    .map_err(|error| {
        StorageError::Other(format!(
            "cannot configure workload secret resolver: {error}"
        ))
    })
}

/// Whether THIS host can resolve every secret the job declares, asked before
/// the claim and without reading a single value.
///
/// A job whose secrets this agent cannot reach must be left in the queue for
/// a host that can, not failed. Until 2026-09-05 the resolution happened
/// after the claim, so `preferences` 0.1.1's `web` build was claimed by this
/// laptop — whose `agent.skarbiec.url` is `http://127.0.0.1:19096` with
/// nothing listening — and the job was FAILED with `cannot resolve job …
/// secret GITHUB_TOKEN: error sending request for url
/// (http://127.0.0.1:19096/v1/items/read)`, while charless-mac-mini, which
/// holds the grant, sat idle. Retrying could not help: placement had already
/// been decided by who was free rather than by who could read.
///
/// `list_items` is metadata only — ids, never values — so this costs one
/// round trip and reveals nothing. It proves both halves that failed: that
/// the broker answers at all, and that this consumer's grant exposes each
/// declared item.
pub(crate) async fn secrets_resolvable_here(job: &Job) -> Result<(), String> {
    if job.secret_env.is_empty() {
        return Ok(());
    }
    let client = agent_secret_client().map_err(|error| error.to_string())?;
    let visible = client
        .list_items()
        .await
        .map_err(|error| format!("the agent's Skarbiec broker did not answer: {error}"))?;
    for (env_name, reference) in &job.secret_env {
        if !visible.iter().any(|item| item.id == reference.item) {
            return Err(format!(
                "secret {env_name} needs item {} and this host's agent grant does not expose it",
                reference.item
            ));
        }
    }
    Ok(())
}

pub(crate) async fn resolve_job_secret_environment(
    job: &Job,
) -> Result<BTreeMap<String, String>, StorageError> {
    let mut environment = BTreeMap::new();
    if job.secret_env.is_empty() {
        return Ok(environment);
    }
    // The grant-file / cached-bearer decision lives in `agent_secret_client`,
    // so the pre-claim probe and this read can never consult a different
    // broker than each other.
    let client = agent_secret_client()?;
    for (env_name, reference) in &job.secret_env {
        if !valid_env_name(env_name)
            || reference.item.trim().is_empty()
            || reference.field.trim().is_empty()
        {
            return Err(StorageError::Other(format!(
                "job {} contains an invalid secret environment reference",
                job.job_id
            )));
        }
        if !crate::config::agent_secret_reference_allowed(&reference.item, &reference.field) {
            return Err(StorageError::Other(format!(
                "job {} secret {env_name} is outside agent.skarbiec.secret_fields",
                job.job_id
            )));
        }
        let value = client
            .read_string(&reference.item, &reference.field)
            .await
            .map_err(|error| {
                StorageError::Other(format!(
                    "cannot resolve job {} secret {env_name}: {error}",
                    job.job_id
                ))
            })?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "job {} secret {env_name} is missing or empty",
                    job.job_id
                ))
            })?;
        environment.insert(env_name.clone(), value);
    }
    Ok(environment)
}
