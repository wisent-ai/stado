//! Fleet inventory: the cross-provider enumeration that turns every live
//! agent VM into a row carrying who still claims it, plus the read-only
//! projection the resource-rationalization commands audit through.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::cli::CmdError;
use crate::config;
use crate::providers::{get_provider, Provider};
use crate::queue::capacity::read_consumer_capacity;
use crate::queue::JobStorage;

use super::holders::Holders;
use super::output::UNKNOWN;

/// One live agent VM plus everything known about who owns it.
pub(super) struct InstanceRow {
    pub(super) reference: String,
    pub(super) provider: String,
    pub(super) accel: String,
    pub(super) age_seconds: f64,
    /// Reasons this VM is still claimed. Empty = orphan.
    pub(super) held_by: Vec<String>,
}

impl InstanceRow {
    pub(super) fn is_orphan(&self) -> bool {
        self.held_by.is_empty()
    }
}

/// The whole cross-provider picture: rows, the provider clients that
/// produced them (reused for deletion), and per-provider enumeration
/// failures. A provider that could not be reached yields an error entry, not
/// an empty fleet — "no VMs" and "no answer" must never look the same to an
/// operator hunting a runaway bill.
pub(super) struct Fleet {
    pub(super) rows: Vec<InstanceRow>,
    clients: BTreeMap<String, Arc<dyn Provider>>,
    pub(super) errors: BTreeMap<String, String>,
}

impl Fleet {
    pub(super) fn rows_for<'a>(
        &'a self,
        provider: &'a str,
    ) -> impl Iterator<Item = &'a InstanceRow> {
        self.rows.iter().filter(move |row| row.provider == provider)
    }
}

pub(super) async fn inventory(store: &JobStorage, providers: &[String]) -> Result<Fleet, CmdError> {
    let live = read_consumer_capacity(store).await?;
    inventory_with_live(store, providers, &live).await
}

async fn inventory_with_live(
    store: &JobStorage,
    providers: &[String],
    live: &BTreeMap<String, Value>,
) -> Result<Fleet, CmdError> {
    let holders = Holders::build(store).await?;
    let agent_prefix = format!("{}-agent-", config::INSTANCE_PREFIX);

    let mut fleet = Fleet {
        rows: Vec::new(),
        clients: BTreeMap::new(),
        errors: BTreeMap::new(),
    };
    for name in providers {
        let client = match get_provider(name) {
            Ok(client) => client,
            Err(err) => {
                fleet.errors.insert(name.clone(), err.to_string());
                continue;
            }
        };
        let refs = match client.list_running_instance_refs_with_age().await {
            Ok(refs) => refs,
            Err(err) => {
                fleet.errors.insert(name.clone(), err.to_string());
                continue;
            }
        };
        fleet.clients.insert(name.clone(), client);
        for (reference, age_seconds) in refs {
            let vm_name = reference.split('@').next().unwrap_or_default().to_string();
            let held_by = holders.holders_for(&reference, &vm_name);
            let accel = holders
                .gpu_type_for(&reference, &vm_name)
                .or_else(|| broadcast_accel(live, name, &vm_name))
                .or_else(|| name_tag_accel(&agent_prefix, &vm_name))
                .unwrap_or_else(|| UNKNOWN.to_string());
            fleet.rows.push(InstanceRow {
                reference,
                provider: name.clone(),
                accel,
                age_seconds,
                held_by,
            });
        }
    }
    // Orphans first, then oldest first: the top of the table is the money.
    fleet.rows.sort_by(|left, right| {
        right
            .is_orphan()
            .cmp(&left.is_orphan())
            .then_with(|| right.age_seconds.total_cmp(&left.age_seconds))
            .then_with(|| left.reference.cmp(&right.reference))
    });
    Ok(fleet)
}

/// Read-only fleet projection for resource rationalization. Unlike the
/// operator list/reaper path, this deliberately skips live-capacity loading,
/// whose stale-record GC would make an audit mutate the store.
pub(crate) async fn audit_inventory(
    store: &JobStorage,
    providers: &[String],
) -> Result<AuditFleet, CmdError> {
    let fleet = inventory_with_live(store, providers, &BTreeMap::new()).await?;
    Ok(AuditFleet {
        rows: fleet
            .rows
            .into_iter()
            .map(|row| AuditInstanceRow {
                reference: row.reference,
                provider: row.provider,
                accel: row.accel,
                age_seconds: row.age_seconds,
                held_by: row.held_by,
            })
            .collect(),
        errors: fleet.errors,
    })
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuditInstanceRow {
    pub reference: String,
    pub provider: String,
    pub accel: String,
    pub age_seconds: f64,
    pub held_by: Vec<String>,
}

impl AuditInstanceRow {
    pub fn is_orphan(&self) -> bool {
        self.held_by.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuditFleet {
    pub rows: Vec<AuditInstanceRow>,
    pub errors: BTreeMap<String, String>,
}

/// Accelerator types from this VM's live capacity broadcast
/// (`queue/capacity.rs`, consumer id `<kind>-<vm name>`). Authoritative for
/// an agent that is still publishing; silent for the dead ones, which is
/// exactly when the name tag below has to answer.
fn broadcast_accel(
    live: &BTreeMap<String, Value>,
    provider: &str,
    vm_name: &str,
) -> Option<String> {
    let available = live
        .get(&format!("{provider}-{vm_name}"))?
        .get("available_accelerators")?
        .as_object()?;
    let joined = available.keys().cloned().collect::<Vec<String>>().join(",");
    (!joined.is_empty()).then_some(joined)
}

/// Last-resort accelerator label: the short tag the dispatcher bakes into
/// the VM name (`scheduler/dispatch/agent.rs` builds
/// `<prefix>-agent-<accel tail>-<tick>-<index>`, so "t4" / "80gb"). Not a
/// full accelerator type — it is what is knowable about a VM whose agent
/// never came up.
fn name_tag_accel(agent_prefix: &str, vm_name: &str) -> Option<String> {
    let tag = vm_name.strip_prefix(agent_prefix)?.split('-').next()?;
    (!tag.is_empty()).then(|| tag.to_string())
}
