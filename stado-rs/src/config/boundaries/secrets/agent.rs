//! Workload-agent Skarbiec coordinates and the backend-messaging item list.
//!
//! Local agents read as `stado`; their vault URL and token file can point to
//! another host. Rented machines receive a separately scoped grant because
//! its bearer is shipped onto hardware Stado does not own.

use std::sync::LazyLock;

use crate::config_file::{expand_tilde, resolve as cfg, resolve_list as cfg_list};

static AGENT_SKARBIEC_URL: LazyLock<String> =
    LazyLock::new(|| cfg("WC_AGENT_SKARBIEC_URL", "agent.skarbiec.url", ""));
static AGENT_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_AGENT_SKARBIEC_CONSUMER",
        "agent.skarbiec.consumer",
        crate::config::skarbiec_consumer(),
    )
});
static AGENT_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("workload-agent-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_AGENT_SKARBIEC_TOKEN_FILE",
        "agent.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});
static AGENT_SKARBIEC_ITEMS: LazyLock<Vec<String>> =
    LazyLock::new(|| env_and_file_list("WC_AGENT_SKARBIEC_ITEMS", "agent.skarbiec.items"));
static AGENT_SKARBIEC_SECRET_FIELDS: LazyLock<Vec<String>> = LazyLock::new(|| {
    env_and_file_list(
        "WC_AGENT_SKARBIEC_SECRET_FIELDS",
        "agent.skarbiec.secret_fields",
    )
});

/// The workload secret lists are the union of the unit's environment and the
/// config file. An agent unit pins its list in its environment, and
/// `release catalog enroll` adds a new product's build secrets to the file;
/// if the environment replaced the file, the added secrets would never reach
/// the agent that runs the product's jobs.
fn env_and_file_list(variable: &str, key: &str) -> Vec<String> {
    let mut entries = cfg_list(variable, key, &[]);
    if std::env::var(variable).is_ok_and(|value| !value.trim().is_empty()) {
        let from_file = crate::config_file::get(key)
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        for entry in from_file.iter().filter_map(|value| value.as_str()) {
            if !entries.iter().any(|known| known == entry) {
                entries.push(entry.to_string());
            }
        }
    }
    entries
}
static BACKEND_MESSAGING_SKARBIEC_ITEMS: LazyLock<Vec<String>> = LazyLock::new(|| {
    cfg_list(
        "WC_BACKEND_MESSAGING_SKARBIEC_ITEMS",
        "backend.messaging.skarbiec.items",
        &[],
    )
});

/// Move a standalone worker's legacy grant into the resident host's workload
/// boundary. The host's default control-plane grant must remain independent.
pub(crate) fn resident_worker_environment_key(name: &str) -> &str {
    use crate::capabilities::{AGENT_SKARBIEC, SECRETS_SKARBIEC};
    for (legacy, workload) in [
        (SECRETS_SKARBIEC.url, AGENT_SKARBIEC.url),
        (SECRETS_SKARBIEC.consumer, AGENT_SKARBIEC.consumer),
        (SECRETS_SKARBIEC.token_file, AGENT_SKARBIEC.token_file),
    ] {
        if name == legacy.env {
            return workload.env;
        }
    }
    name
}

/// Skarbiec endpoint reachable by workload agents. Cloud agents require HTTPS;
/// a device-local agent may leave this empty and use
/// [`crate::config::skarbiec_url`].
pub fn agent_skarbiec_url() -> &'static str {
    AGENT_SKARBIEC_URL.as_str()
}

/// Consumer the agent reads workload secrets as: `stado`, or the scoped
/// `*-agent` consumer whose bearer is projected into rented machines.
pub fn agent_skarbiec_consumer() -> &'static str {
    AGENT_SKARBIEC_CONSUMER.as_str()
}

/// Owner-only file containing the workload-agent grant. Cloud deployment
/// delivers the token to VM tmpfs; the local control plane uses a separate
/// device grant rather than reusing its coordinator grant.
pub fn agent_skarbiec_token_file() -> &'static str {
    AGENT_SKARBIEC_TOKEN_FILE.as_str()
}
/// Exact Skarbiec items visible to workload agents. The coordinator verifies
/// that the scoped grant can list neither fewer nor more items before dispatch.
pub fn agent_skarbiec_items() -> &'static [String] {
    &AGENT_SKARBIEC_ITEMS
}

/// Exact workload-visible `item#field` references. Infrastructure items may
/// still be present in [`agent_skarbiec_items`] for trusted agent internals,
/// but a queued job can resolve only entries in this second, field-level list.
pub fn agent_skarbiec_secret_fields() -> &'static [String] {
    &AGENT_SKARBIEC_SECRET_FIELDS
}

/// Backend messaging items Stado resolves for the operator session through
/// its own Skarbiec identity.
pub fn backend_messaging_skarbiec_items() -> &'static [String] {
    &BACKEND_MESSAGING_SKARBIEC_ITEMS
}

/// Whether a job may project one exact Skarbiec field into its environment.
/// Matching without allocating keeps this check cheap on every admission path;
/// only a reference the loaded list lacks is looked up again in the config
/// file as it is now, so a product enrolled after this agent started is
/// served without restarting it.
pub fn agent_secret_reference_allowed(item: &str, field: &str) -> bool {
    let matches = |entry: &str| {
        entry
            .split_once('#')
            .is_some_and(|(allowed_item, allowed_field)| {
                allowed_item == item && allowed_field == field
            })
    };
    AGENT_SKARBIEC_SECRET_FIELDS
        .iter()
        .any(|entry| matches(entry))
        || crate::config_file::get_fresh("agent.skarbiec.secret_fields")
            .and_then(|value| value.as_array().cloned())
            .is_some_and(|entries| {
                entries
                    .iter()
                    .filter_map(|value| value.as_str())
                    .any(matches)
            })
}
