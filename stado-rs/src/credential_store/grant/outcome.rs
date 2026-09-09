//! What one call settled, and the one question callers ask of it.

use std::path::PathBuf;

/// What one call settled, for callers that report it.
#[derive(Clone, Debug)]
pub struct GrantOutcome {
    /// Capabilities the consumer held before the mint.
    pub held_before: usize,
    /// Capabilities the consumer holds now.
    pub held_after: usize,
    /// Capabilities this call added; empty means the grant already covered the
    /// request and nothing was written.
    pub added: Vec<String>,
    /// Seconds left on the preserved TTL.
    pub expires_in: i64,
    /// Vault copy taken before the mint, absent when nothing was written.
    pub backup: Option<PathBuf>,
}

impl GrantOutcome {
    /// Whether this call changed the vault.
    pub fn wrote(&self) -> bool {
        !self.added.is_empty()
    }
}
