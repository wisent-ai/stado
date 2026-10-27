//! Vast credential resolution: `vast/api_key` read through this host's
//! Skarbiec channel ([`crate::providers::host_credential`]), the refusal a
//! command that needs the key answers with, and the availability probe the
//! CLI gates the auto-list bridge on.
//!
//! `stado market readiness --provider vast` turns the reading plus the
//! vault's own answer into one verdict.

use crate::providers::host_credential::{read_role_field, HostCredentialReading};

/// The role whose item the bridge reads (the key is read by role, never by
/// item id), and the field on it.
pub const VAST_ROLE: &str = "vast";
pub const VAST_FIELD: &str = "api_key";

/// Read the Vast API key from Skarbiec, keeping what was observed.
pub async fn read_vast_api_key() -> HostCredentialReading {
    read_role_field(VAST_ROLE, VAST_FIELD).await
}

/// The refusal a command that needs the Vast API answers with. It names the
/// channel, quotes what Skarbiec said, and points at the command that
/// resolves which provisioning step is missing.
pub fn vast_refusal(reading: &HostCredentialReading) -> String {
    format!(
        "no Vast.ai API key on this host: {}. This machine cannot list \
         capacity until vast/api_key resolves; `stado market readiness --provider \
         vast` reports whether the fleet vault declares the item and which grant \
         is missing",
        reading.cause()
    )
}

/// Whether this host can present a Vast API key at all. The agent gates its
/// automatic bridge on this.
pub async fn vast_api_key_available() -> bool {
    read_vast_api_key().await.key.is_some()
}
