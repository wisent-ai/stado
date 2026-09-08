//! The payload one host states, and the report the command answers with.

use serde::{Deserialize, Serialize};

use super::reads::{clamp, clamp_cargo_inventory, clamp_vault_section};
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
    /// How many active vaults matched, including any past
    /// [`MAX_VAULT_FILES`] that `vaults` therefore does not list.
    pub vaults_seen: u64,
    /// Everything else under `$HOME/.stado/*.vault*.json`: snapshots,
    /// pre-migration copies, `*.acquisitions.json`. History, not state.
    pub vault_sidecars: Vec<VaultFile>,
    /// How many sidecars matched, including any past [`MAX_VAULT_FILES`].
    pub vault_sidecars_seen: u64,
}

/// Cap every string in the inventory.
fn clamp_inventory(inventory: &mut Inventory) {
    clamp(&mut inventory.forwards_dir_state);
    clamp(&mut inventory.release_platform);
    clamp(&mut inventory.sanitizer_state);
    clamp(&mut inventory.listeners_state);
    for binary in &mut inventory.managed_binaries {
        clamp(&mut binary.name);
        clamp(&mut binary.state);
        clamp(&mut binary.version_state);
        clamp(&mut binary.version);
    }
    for marker in &mut inventory.forwards {
        clamp(&mut marker.name);
        clamp(&mut marker.state);
        clamp(&mut marker.url);
    }
    for listener in &mut inventory.listeners {
        clamp(&mut listener.address);
    }
    if inventory.sanitizer_state != SANITIZER_OK {
        inventory.cargo.entries_complete = false;
        inventory.cargo.complete = false;
    }
    for subcommand in &mut inventory.subcommands {
        clamp(&mut subcommand.name);
        clamp(&mut subcommand.state);
    }
    clamp_vault_section(&mut inventory.vaults, &mut inventory.vaults_seen);
    clamp_vault_section(
        &mut inventory.vault_sidecars,
        &mut inventory.vault_sidecars_seen,
    );
    clamp_cargo_inventory(&mut inventory.cargo);
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
        .ok_or_else(|| DeployError("host inventory script produced no JSON report".to_string()))?;
    let mut inventory: Inventory = serde_json::from_str(payload).map_err(|error| {
        DeployError(format!(
            "host inventory script did not return the expected JSON: {error}"
        ))
    })?;
    clamp_inventory(&mut inventory);
    Ok(inventory)
}
