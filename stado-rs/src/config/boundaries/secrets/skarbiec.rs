//! The control-plane Skarbiec endpoint, grant and declared owner vault.

use std::sync::LazyLock;

use crate::config_file::{expand_tilde, resolve as cfg};

static SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_SKARBIEC_URL",
        "secrets.skarbiec.url",
        "http://127.0.0.1:17602",
    )
});
static SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_SKARBIEC_CONSUMER",
        "secrets.skarbiec.consumer",
        "stado-control-plane",
    )
});
static SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("control-plane-skarbiec-token")
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

/// Loopback URL of the separate Skarbiec service.
pub fn skarbiec_url() -> &'static str {
    SKARBIEC_URL.as_str()
}

/// Scoped Skarbiec grant consumer name.
pub fn skarbiec_consumer() -> &'static str {
    SKARBIEC_CONSUMER.as_str()
}

/// Owner-only file containing the scoped Skarbiec grant.
pub fn skarbiec_token_file() -> &'static str {
    SKARBIEC_TOKEN_FILE.as_str()
}

/// The declared owner vault on this machine, empty when nothing declares one.
pub fn skarbiec_vault_file() -> &'static str {
    SKARBIEC_VAULT_FILE.as_str()
}
