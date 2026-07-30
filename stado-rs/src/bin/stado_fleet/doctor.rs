//! `stado_fleet doctor` — one command that answers "is every registered
//! worker able to run, and if not, why".
//!
//! Three read-only sections, all through Stado's own surfaces:
//! - the local agent credential grant vs the configured allowlist, plus a
//!   probe of every declared secret field (values are never printed),
//! - per-target health beacons from the store,
//! - live capacity broadcasts vs registered local targets.
//!
//! Anything a worker needs but cannot get marks the check failed; the
//! process exit code carries the verdict for automation.

use serde_json::Value;
use stado::config;
use stado::monitor::host_health;
use stado::queue::{self, JobStorage};
use stado::skarbiec::Client;
use stado::targets;

/// One named probe result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: String,
    pub ok: bool,
    pub detail: String,
}

fn pass(id: &str, detail: String) -> Check {
    Check {
        id: id.to_string(),
        ok: true,
        detail,
    }
}

fn fail(id: &str, detail: String) -> Check {
    Check {
        id: id.to_string(),
        ok: false,
        detail,
    }
}

/// Items the grant must expose per config vs what it actually exposes:
/// `(missing, extra)`, both sorted. Pure — covered by unit tests.
pub fn grant_drift(expected: &[String], visible: &[String]) -> (Vec<String>, Vec<String>) {
    let mut missing: Vec<String> = expected
        .iter()
        .filter(|item| !visible.contains(item))
        .cloned()
        .collect();
    let mut extra: Vec<String> = visible
        .iter()
        .filter(|item| !expected.contains(item))
        .cloned()
        .collect();
    missing.sort();
    extra.sort();
    (missing, extra)
}

/// Split an `item#field` allowlist entry; anything without both halves is
/// refused. Pure — covered by unit tests.
pub fn parse_secret_field(entry: &str) -> Option<(&str, &str)> {
    let (item, field) = entry.split_once('#')?;
    if item.is_empty() || field.is_empty() {
        return None;
    }
    Some((item, field))
}

/// The check that would have named today's crash loop: can the local agent
/// consumer list its grant, does the grant match `agent.skarbiec.items`,
/// and does every declared `item#field` actually read back.
async fn agent_grant_checks() -> Vec<Check> {
    // Same URL resolution the runtime uses (providers/local/slots.rs):
    // an unset agent.skarbiec.url means the shared Stado skarbiec URL.
    let configured_url = config::agent_skarbiec_url();
    let url = if configured_url.trim().is_empty() {
        config::skarbiec_url()
    } else {
        configured_url
    };
    let consumer = config::agent_skarbiec_consumer();
    let token_file = config::agent_skarbiec_token_file();
    if consumer.is_empty() || token_file.is_empty() {
        return vec![fail(
            "agent-grant",
            "agent.skarbiec is not configured (consumer/token_file missing)".to_string(),
        )];
    }
    let client = match Client::new(url, consumer, token_file) {
        Ok(client) => client,
        Err(exc) => return vec![fail("agent-grant", exc.to_string())],
    };
    let mut checks = Vec::new();
    match client.list_items().await {
        Err(exc) => checks.push(fail(
            "agent-grant-list",
            format!("grant listing failed for consumer {consumer}: {exc}"),
        )),
        Ok(items) => {
            let visible: Vec<String> = items.into_iter().map(|item| item.id).collect();
            let (missing, extra) = grant_drift(config::agent_skarbiec_items(), &visible);
            if missing.is_empty() && extra.is_empty() {
                checks.push(pass(
                    "agent-grant-drift",
                    format!("grant matches agent.skarbiec.items ({consumer})"),
                ));
            } else {
                checks.push(fail(
                    "agent-grant-drift",
                    format!("grant drift for {consumer}: missing={missing:?} extra={extra:?}"),
                ));
            }
        }
    }
    let mut unreadable: Vec<String> = Vec::new();
    let mut probed: Vec<String> = Vec::new();
    for entry in config::agent_skarbiec_secret_fields() {
        let Some((item, field)) = parse_secret_field(entry) else {
            unreadable.push(format!("{entry} (malformed allowlist entry)"));
            continue;
        };
        probed.push(format!("{item}#{field}"));
        match client.read_string(item, field).await {
            Ok(Some(_)) => {}
            Ok(None) => unreadable.push(format!("{item}#{field} (absent or empty)")),
            Err(exc) => unreadable.push(format!("{item}#{field} ({exc})")),
        }
    }
    if unreadable.is_empty() {
        checks.push(pass(
            "agent-secret-probes",
            format!("every declared secret field reads back: {}", probed.join(", ")),
        ));
    } else {
        checks.push(fail(
            "agent-secret-probes",
            format!("unreadable secret fields: {}", unreadable.join(", ")),
        ));
    }
    checks
}

/// Per-target beacon presence plus live capacity vs the registered local
/// targets. A registered target with no live capacity broadcast is a worker
/// that is down, whatever the reason — that is what the fleet manager must
/// say out loud. With `scoped`, only members of that named fleet are
/// checked; an undeclared fleet name is an error, not an empty pass.
async fn fleet_checks(store: &JobStorage, scoped: Option<&str>) -> Result<Vec<Check>, String> {
    let document = stado::cli::registry::fetch_document()
        .await
        .map_err(|exc| exc.to_string())?;
    let fleets = crate::fleet::parse_fleets(&document)?;
    let wanted: Option<Vec<String>> = match scoped {
        Some(name) => Some(
            crate::fleet::find_fleet(&fleets, name)
                .ok_or_else(|| format!("fleet '{name}' is not declared in the registry"))?
                .members
                .clone(),
        ),
        None => None,
    };
    let registry = targets::load_registry_auto()
        .await
        .map_err(|exc| exc.to_string())?;
    let consumers = queue::capacity::read_consumer_capacity(store)
        .await
        .map_err(|exc| exc.to_string())?;
    let broadcasting: Vec<String> = consumers.keys().cloned().collect();
    let mut checks = Vec::new();
    for target in registry.local_targets() {
        if let Some(members) = &wanted {
            if !members.iter().any(|member| member == &target.name) {
                continue;
            }
        }
        match host_health::load_host_health(store, &target.name).await {
            Ok(report) => {
                let reported_at = report
                    .beacon
                    .get("reported_at")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                checks.push(pass(
                    "beacon",
                    format!("{}: last beacon at {reported_at}", target.name),
                ));
            }
            Err(exc) => checks.push(fail(
                "beacon",
                format!("{}: no readable health beacon ({exc})", target.name),
            )),
        }
    }
    if broadcasting.is_empty() {
        checks.push(fail(
            "capacity",
            "no consumer is broadcasting capacity; the fleet cannot claim work".to_string(),
        ));
    } else {
        checks.push(pass(
            "capacity",
            format!("broadcasting consumers: {}", broadcasting.join(", ")),
        ));
    }
    Ok(checks)
}

/// Run every section, print the report, and return whether the fleet is
/// clean. Read-only: nothing here mutates the store, the registry, or any
/// credential. `scoped` limits the beacon section to one named fleet; the
/// agent-grant section always covers this machine's own worker grant.
pub async fn run(as_json: bool, scoped: Option<&str>) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let mut checks = agent_grant_checks().await;
    checks.extend(fleet_checks(&store, scoped).await?);
    let clean = checks.iter().all(|check| check.ok);
    if as_json {
        let document: Value = serde_json::json!({
            "clean": clean,
            "checks": checks.iter().map(|check| serde_json::json!({
                "id": check.id,
                "ok": check.ok,
                "detail": check.detail,
            })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(|exc| exc.to_string())?
        );
    } else {
        for check in &checks {
            let status = if check.ok { "PASS" } else { "FAIL" };
            println!("{status}\t{}\t{}", check.id, check.detail);
        }
        println!(
            "\nstado-fleet doctor: {}",
            if clean {
                "fleet is clean"
            } else {
                "fleet has failing checks"
            }
        );
    }
    Ok(clean)
}
