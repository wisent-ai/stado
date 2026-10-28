//! Workload-agent Skarbiec coordinates and the backend-messaging item list.
//!
//! Local agents read as `stado`; their vault URL and token file can point to
//! another host. Rented machines receive a separately scoped grant because
//! its bearer is shipped onto hardware Stado does not own.

use std::sync::LazyLock;

use serde_json::Value;

use crate::config_file::{expand_tilde, resolve as cfg, resolve_list as cfg_list};

/// The agent Skarbiec URL as the configuration writes it, unresolved: what a
/// unit installed on this host carries when the service it names cannot be
/// resolved now, so the unit's own process resolves it when it reads it.
static AGENT_SKARBIEC_URL_CONFIGURED: LazyLock<String> =
    LazyLock::new(|| cfg("WC_AGENT_SKARBIEC_URL", "agent.skarbiec.url", ""));
/// A configured `stado://service/skarbiec?consumer=<consumer>` is that
/// adapter's address as this host's resolver published it; one that does
/// not resolve keeps the cause, so no reader takes it for an unset URL.
static AGENT_SKARBIEC_URL: LazyLock<Result<String, String>> =
    LazyLock::new(|| crate::service_resolution::local_address(&AGENT_SKARBIEC_URL_CONFIGURED));
static AGENT_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_AGENT_SKARBIEC_CONSUMER",
        "agent.skarbiec.consumer",
        crate::config::skarbiec_consumer(),
    )
});
static AGENT_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    expand_tilde(&cfg(
        "WC_AGENT_SKARBIEC_TOKEN_FILE",
        "agent.skarbiec.token_file",
        "~/.stado/workload-agent-skarbiec-token",
    ))
    .to_string_lossy()
    .into_owned()
});
static AGENT_SKARBIEC_ROLES: LazyLock<Vec<String>> =
    LazyLock::new(|| env_and_file_list("WC_AGENT_SKARBIEC_ROLES", "agent.skarbiec.roles"));
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
        // A file without the list, or with it as another type, adds
        // nothing to the environment's; `config validate` names a wrong type.
        let from_file: Vec<Value> = match crate::config_file::get(key) {
            Some(Value::Array(entries)) => entries,
            _ => Vec::new(),
        };
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
/// [`crate::config::skarbiec_url`]. A configured service that this host
/// cannot resolve answers empty too, with its cause in
/// [`agent_skarbiec_url_absent`], as [`crate::config::skarbiec_url`] does:
/// a reader that would fall back to the shared Skarbiec on empty asks that
/// first, so an unresolved service is never read as an unset URL.
pub fn agent_skarbiec_url() -> &'static str {
    match AGENT_SKARBIEC_URL.as_ref() {
        Ok(url) => url,
        Err(_explained_by_agent_skarbiec_url_absent) => "",
    }
}

/// Why the configured agent Skarbiec service cannot be resolved on this
/// host; `None` when the URL resolved or is not a named service.
pub fn agent_skarbiec_url_absent() -> Option<&'static str> {
    AGENT_SKARBIEC_URL.as_ref().err().map(String::as_str)
}

/// The agent Skarbiec URL as configured, before a named service is resolved.
pub fn agent_skarbiec_url_configured() -> &'static str {
    AGENT_SKARBIEC_URL_CONFIGURED.as_str()
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
/// The roles whose items workload agents may see (`skarbiec::roles`). The
/// coordinator verifies that the scoped grant can list exactly one item per
/// role and nothing else before dispatch.
pub fn agent_skarbiec_roles() -> &'static [String] {
    &AGENT_SKARBIEC_ROLES
}

/// Exact workload-visible `role#field` references. A role may be present in
/// [`agent_skarbiec_roles`] for trusted agent internals, but a queued job can
/// resolve only entries in this second, field-level list.
pub fn agent_skarbiec_secret_fields() -> &'static [String] {
    &AGENT_SKARBIEC_SECRET_FIELDS
}

/// [`agent_skarbiec_secret_fields`] joined with the config file as it is now:
/// what this agent can serve at this moment, the same reading
/// [`agent_secret_reference_allowed`] grants by. `release catalog enroll`
/// writes a new product's build secret into the file of a running agent; a
/// capacity publication of the startup list alone hid it from every
/// coordinator until the agent restarted.
pub fn agent_skarbiec_secret_fields_now() -> Result<Vec<String>, String> {
    let mut fields = AGENT_SKARBIEC_SECRET_FIELDS.clone();
    for entry in fresh_secret_fields()? {
        if !fields.contains(&entry) {
            fields.push(entry);
        }
    }
    Ok(fields)
}

/// The `agent.skarbiec.secret_fields` list the config file holds now. A file
/// that names no list enrolls nothing beyond the startup list. A file that
/// cannot be read, a list of another type, or an entry that is not text is
/// refused naming it: read as nothing, it withheld every secret enrolled
/// after start and said only that the job was not allowed.
fn fresh_secret_fields() -> Result<Vec<String>, String> {
    match crate::config_file::get_fresh("agent.skarbiec.secret_fields")? {
        None => Ok(Vec::new()),
        Some(Value::Array(entries)) => entries
            .iter()
            .map(|entry| match entry {
                Value::String(reference) => Ok(reference.clone()),
                other => Err(format!(
                    "agent.skarbiec.secret_fields holds {other}, which is not a role#field reference"
                )),
            })
            .collect(),
        Some(other) => Err(format!(
            "agent.skarbiec.secret_fields is {other}, not a list of role#field references"
        )),
    }
}

/// Backend messaging items Stado resolves for the operator session through
/// its own Skarbiec identity.
pub fn backend_messaging_skarbiec_items() -> &'static [String] {
    &BACKEND_MESSAGING_SKARBIEC_ITEMS
}

/// Whether a job may project one field of one role into its environment.
/// Matching without allocating keeps this check cheap on every admission path;
/// only a reference the loaded list lacks is looked up again in the config
/// file as it is now, so a product enrolled after this agent started is
/// served without restarting it. A config file that cannot be read now is
/// the refusal, not a "no".
pub fn agent_secret_reference_allowed(role: &str, field: &str) -> Result<bool, String> {
    let matches = |entry: &str| {
        entry
            .split_once('#')
            .is_some_and(|(allowed_role, allowed_field)| {
                allowed_role == role && allowed_field == field
            })
    };
    if AGENT_SKARBIEC_SECRET_FIELDS
        .iter()
        .any(|entry| matches(entry))
    {
        return Ok(true);
    }
    Ok(fresh_secret_fields()?.iter().any(|entry| matches(entry)))
}
