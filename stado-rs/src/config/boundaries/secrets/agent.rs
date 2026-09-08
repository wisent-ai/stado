//! Workload-agent and backend-messaging Skarbiec grants.

use std::sync::LazyLock;

use crate::config::skarbiec_url;
use crate::config_file::{expand_tilde, resolve as cfg, resolve_list as cfg_list};

static AGENT_SKARBIEC_URL: LazyLock<String> =
    LazyLock::new(|| cfg("WC_AGENT_SKARBIEC_URL", "agent.skarbiec.url", ""));
static AGENT_SKARBIEC_CONSUMER: LazyLock<String> =
    LazyLock::new(|| cfg("WC_AGENT_SKARBIEC_CONSUMER", "agent.skarbiec.consumer", ""));
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
    LazyLock::new(|| cfg_list("WC_AGENT_SKARBIEC_ITEMS", "agent.skarbiec.items", &[]));
static AGENT_SKARBIEC_SECRET_FIELDS: LazyLock<Vec<String>> = LazyLock::new(|| {
    cfg_list(
        "WC_AGENT_SKARBIEC_SECRET_FIELDS",
        "agent.skarbiec.secret_fields",
        &[],
    )
});
static BACKEND_MESSAGING_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_BACKEND_MESSAGING_SKARBIEC_URL",
        "backend.messaging.skarbiec.url",
        skarbiec_url(),
    )
});
static BACKEND_MESSAGING_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_BACKEND_MESSAGING_SKARBIEC_CONSUMER",
        "backend.messaging.skarbiec.consumer",
        "",
    )
});
static BACKEND_MESSAGING_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    expand_tilde(&cfg(
        "WC_BACKEND_MESSAGING_SKARBIEC_TOKEN_FILE",
        "backend.messaging.skarbiec.token_file",
        "",
    ))
    .to_string_lossy()
    .into_owned()
});
static BACKEND_MESSAGING_SKARBIEC_ITEMS: LazyLock<Vec<String>> = LazyLock::new(|| {
    cfg_list(
        "WC_BACKEND_MESSAGING_SKARBIEC_ITEMS",
        "backend.messaging.skarbiec.items",
        &[],
    )
});

/// Skarbiec endpoint reachable by workload agents. Cloud agents require HTTPS;
/// a device-local agent may leave this empty and use [`skarbiec_url`].
pub fn agent_skarbiec_url() -> &'static str {
    AGENT_SKARBIEC_URL.as_str()
}

/// Consumer name of the dedicated workload-agent grant. Its exact read scopes
/// are minted from the workloads this deployment is allowed to execute.
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

/// HTTPS Skarbiec endpoint of the backend business-messaging grant, which
/// Stado reads only to resolve the operator-session Supabase project.
pub fn backend_messaging_skarbiec_url() -> &'static str {
    BACKEND_MESSAGING_SKARBIEC_URL.as_str()
}

/// Dedicated consumer whose grant contains only backend messaging providers.
pub fn backend_messaging_skarbiec_consumer() -> &'static str {
    BACKEND_MESSAGING_SKARBIEC_CONSUMER.as_str()
}

/// Owner-only grant file for the backend business-messaging grant.
pub fn backend_messaging_skarbiec_token_file() -> &'static str {
    BACKEND_MESSAGING_SKARBIEC_TOKEN_FILE.as_str()
}

/// Exact provider and device-registry items visible to the messaging grant.
pub fn backend_messaging_skarbiec_items() -> &'static [String] {
    &BACKEND_MESSAGING_SKARBIEC_ITEMS
}

/// Whether a job may project one exact Skarbiec field into its environment.
/// Matching without allocating keeps this check cheap on every admission path.
pub fn agent_secret_reference_allowed(item: &str, field: &str) -> bool {
    AGENT_SKARBIEC_SECRET_FIELDS.iter().any(|entry| {
        entry
            .split_once('#')
            .is_some_and(|(allowed_item, allowed_field)| {
                allowed_item == item && allowed_field == field
            })
    })
}
