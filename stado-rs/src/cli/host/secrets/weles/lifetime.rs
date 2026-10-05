//! How long the grants of a registered acquisition catalog live.
//!
//! Skarbiec states no lifetime of its own: `token-register-acquisitions`
//! requires `--ttl-seconds`, the grants' lifetime in whole seconds, and the
//! 30 days it used to assume are gone. Registration is repeated on every
//! `service ensure`, so a lifetime taken from anywhere but the operator would
//! extend the grants each time, which is what re-minting used to do silently.
//! The lifetime is therefore, in order: the one the operator states, or the
//! remaining life of the registration the catalog already has, so that
//! re-registering keeps the expiry it found (the rule `grant rebind` follows).
//! A catalog with no current registration and no stated lifetime is refused
//! by name rather than given a number nobody chose.

use serde_json::Value;

use crate::targets::ComputeTarget;

/// The consumers a catalog's rows register, each once.
fn catalog_consumers(catalog: &str) -> Vec<&str> {
    let mut consumers: Vec<&str> = Vec::new();
    for line in catalog.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(consumer) = line.split('|').next().filter(|word| !word.is_empty()) {
            if !consumers.contains(&consumer) {
                consumers.push(consumer);
            }
        }
    }
    consumers
}

/// The lifetime to register CATALOG (a path on the host) with: STATED when
/// the operator gave one, otherwise the seconds left on the earliest
/// unexpired grant among the catalog's consumers in VAULT.
pub(super) async fn registration_lifetime(
    resolved: &ComputeTarget,
    catalog: &str,
    vault: &str,
    skarbiec: &str,
    stated: Option<u64>,
    runner: &crate::deploy::Runner,
) -> Result<u64, String> {
    if let Some(seconds) = stated {
        return Ok(seconds);
    }
    use crate::deploy::host_channel;
    let text = host_channel::remote_read_file(resolved, catalog, runner)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the catalog {catalog} could not be read back"))?;
    let consumers = catalog_consumers(&text);
    let listed = host_channel::run_command(
        resolved,
        &format!(
            "SKARBIEC_VAULT_FILE={} {} grant list",
            crate::deploy::shlex_quote(vault),
            crate::deploy::shlex_quote(skarbiec),
        ),
        runner,
    )
    .await
    .map_err(|error| error.to_string())?;
    if !listed.ok() {
        return Err(host_channel::last_error_line(
            &listed,
            "skarbiec grant list failed",
        ));
    }
    let grants: Vec<Value> = serde_json::from_str(listed.stdout.trim())
        .map_err(|error| format!("skarbiec grant list printed no readable JSON: {error}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    grants
        .iter()
        .filter(|grant| {
            grant["consumer"]
                .as_str()
                .is_some_and(|consumer| consumers.contains(&consumer))
        })
        .filter_map(|grant| grant["expires_at"].as_u64())
        .filter(|expires_at| *expires_at > now)
        .min()
        .map(|expires_at| expires_at - now)
        .ok_or_else(|| {
            format!(
                "no grant of this catalog is current in {vault}, so there is no lifetime on record \
                 to keep, and Skarbiec requires one: state it with `stado credentials \
                 acquisition-scopes sync --host {} <catalog> --ttl-seconds <seconds>`",
                resolved.name
            )
        })
}
