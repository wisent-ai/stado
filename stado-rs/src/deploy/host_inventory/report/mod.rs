//! The payload one host states, and the report the command answers with.

use serde::{Deserialize, Serialize};

use super::reads::settle_cargo_inventory;
use super::*;
use crate::deploy::DeployError;

mod assembly;
mod collect;

pub use assembly::to_report;
pub use collect::{inventory_host, inventory_target};

/// Everything the remote script reported, before reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    /// [`SANITIZER_OK`] or [`SANITIZER_BROKEN`], from the script's check of
    /// its own sanitizer against a fixed probe. Every string below went
    /// through that sanitizer, so this is the field that says whether any of
    /// them can be believed.
    pub sanitizer_state: String,
    /// Platform derived from the remote kernel and architecture.
    pub release_platform: String,
    pub forwards_dir_state: String,
    pub managed_binaries: Vec<ManagedBinary>,
    /// What the service units are actually executing, and how old it is
    /// beside the installed program of the same name. Empty on a host with
    /// no `$HOME/.stado/services` tree.
    #[serde(default)]
    pub service_artifacts: Vec<ServiceArtifact>,
    /// Metadata for `$HOME/.cargo` and `$HOME/.cargo/bin`, plus the bin
    /// directory's complete child membership when `entries_complete` is true.
    #[serde(default)]
    pub cargo: CargoInventory,
    pub forwards: Vec<ForwardMarker>,
    pub listeners: Vec<Listener>,
    /// [`LISTENERS_READ`] or [`LISTENERS_FAILED`]. An empty `listeners` means
    /// two very different things depending on this, and reconciling markers
    /// against a table that was never read is how one failed `netstat`
    /// becomes a report that every forward on the host is stale.
    pub listeners_state: String,
    pub subcommands: Vec<Subcommand>,
    /// The active vaults: exactly `$HOME/.stado/*.vault.json`.
    pub vaults: Vec<VaultFile>,
    /// How many active vaults the script matched; `vaults` lists each.
    pub vaults_seen: u64,
    /// Everything else under `$HOME/.stado/*.vault*.json`: snapshots,
    /// pre-migration copies, `*.acquisitions.json`. History, not state.
    pub vault_sidecars: Vec<VaultFile>,
    /// How many sidecars the script matched; `vault_sidecars` lists each.
    pub vault_sidecars_seen: u64,
}

/// Settle what the counts and completeness flags say about the lists that
/// arrived. Nothing is cut: every string and every list is reported whole,
/// and the script's sanitizer already made each value JSON-inert. The
/// per-string, vault-file and Cargo-entry caps were nobody's statement.
fn settle_inventory(inventory: &mut Inventory) {
    if inventory.sanitizer_state != SANITIZER_OK {
        inventory.cargo.entries_complete = false;
        inventory.cargo.complete = false;
    }
    inventory.vaults_seen = inventory.vaults_seen.max(inventory.vaults.len() as u64);
    inventory.vault_sidecars_seen = inventory
        .vault_sidecars_seen
        .max(inventory.vault_sidecars.len() as u64);
    settle_cargo_inventory(&mut inventory.cargo);
}

/// Parse the script's one line of JSON.
///
/// The LAST line starting with `{` is the payload: a login shell that
/// greets its callers must not be able to turn a healthy host into a parse
/// error.
pub fn parse_inventory(stdout: &str) -> Result<Inventory, DeployError> {
    let payload = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| {
            DeployError::unreachable("host inventory script produced no JSON report".to_string())
        })?;
    let mut inventory: Inventory = serde_json::from_str(payload).map_err(|error| {
        DeployError::unreachable(format!(
            "host inventory script did not return the expected JSON: {error}"
        ))
    })?;
    settle_inventory(&mut inventory);
    Ok(inventory)
}
