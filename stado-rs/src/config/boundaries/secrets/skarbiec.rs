//! Stado's one Skarbiec identity: endpoint, consumer, bearer file and the
//! declared owner vault.
//!
//! Stado presents itself to the vault as `stado`. Its API boundaries use
//! these accessors for the endpoint and bearer file.

use std::sync::LazyLock;

use crate::config_file::{expand_tilde, resolve as cfg};

/// Where this machine reaches Skarbiec: `WC_SKARBIEC_URL`, then the host
/// config's `secrets.skarbiec.url`, then the address `stado service directory
/// publish` wrote for this host into `~/.stado/forwards/skarbiec.local`. No
/// address is built in: a host the directory gives no Skarbiec endpoint
/// answers empty, and every reader refuses with that instead of dialing a
/// port nothing may serve.
static SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    let configured = cfg("WC_SKARBIEC_URL", "secrets.skarbiec.url", "");
    if !configured.trim().is_empty() {
        return configured;
    }
    crate::deploy::host_access::forward::read_local("skarbiec")
        .ok()
        .flatten()
        .map(|marker| marker.url)
        .unwrap_or_default()
});
static SKARBIEC_CONSUMER: LazyLock<String> =
    LazyLock::new(|| cfg("WC_SKARBIEC_CONSUMER", "secrets.skarbiec.consumer", "stado"));
static SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_SKARBIEC_TOKEN_FILE",
        "secrets.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});
/// The vault this machine's owner writes go through, declared rather than
/// discovered. Empty means "discover", which is correct on a host holding one
/// vault and refused on a host holding two under one owner.
static SKARBIEC_VAULT_FILE: LazyLock<String> = LazyLock::new(|| {
    let declared = cfg("SKARBIEC_VAULT_FILE", "secrets.skarbiec.vault_file", "");
    if declared.trim().is_empty() {
        return String::new();
    }
    expand_tilde(declared.trim()).to_string_lossy().into_owned()
});

/// The address of the Skarbiec this machine uses; empty when neither the
/// environment, the host config nor the service directory names one.
pub fn skarbiec_url() -> &'static str {
    SKARBIEC_URL.as_str()
}

/// Stado's Skarbiec consumer name: `stado`.
pub fn skarbiec_consumer() -> &'static str {
    SKARBIEC_CONSUMER.as_str()
}

/// Owner-only file holding Stado's Skarbiec bearer.
pub fn skarbiec_token_file() -> &'static str {
    SKARBIEC_TOKEN_FILE.as_str()
}

/// The declared owner vault on this machine, empty when nothing declares one.
pub fn skarbiec_vault_file() -> &'static str {
    SKARBIEC_VAULT_FILE.as_str()
}
