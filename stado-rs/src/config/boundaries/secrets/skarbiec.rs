//! Stado's one Skarbiec identity: endpoint, consumer, bearer file and the
//! declared owner vault.
//!
//! Stado presents itself to the vault as `stado`. Its API boundaries use
//! these accessors for the endpoint and bearer file.

use std::sync::LazyLock;

use crate::config_file::{expand_tilde, resolve as cfg};

/// Where this machine reaches Skarbiec: `WC_SKARBIEC_URL`, then the host
/// config's `secrets.skarbiec.url`, then the address `stado service directory
/// publish` wrote for this host into `~/.stado/forwards/skarbiec.local`. A
/// configured `stado://service/skarbiec?consumer=<consumer>` is that
/// adapter's address as this host's resolver published it. No address is
/// built in: a host the directory gives no Skarbiec endpoint has none, and
/// the cause is kept beside it — nothing names one, or the marker that
/// would cannot be read — so a reader refuses with that cause instead of
/// dialing a port nothing may serve or calling an empty address invalid.
static SKARBIEC_URL: LazyLock<Result<String, String>> = LazyLock::new(|| {
    let configured = crate::service_resolution::local_address(&cfg(
        "WC_SKARBIEC_URL",
        "secrets.skarbiec.url",
        "",
    ))?;
    if !configured.trim().is_empty() {
        return Ok(configured);
    }
    match crate::deploy::host_access::forward::read_local("skarbiec") {
        Ok(Some(marker)) => Ok(marker.url),
        Ok(None) => Err(
            "neither WC_SKARBIEC_URL, the host config's secrets.skarbiec.url nor this host's \
             ~/.stado/forwards/skarbiec.local (written by `stado service directory publish`) \
             names one"
                .to_string(),
        ),
        Err(error) => Err(error.message),
    }
});
static SKARBIEC_CONSUMER: LazyLock<String> =
    LazyLock::new(|| cfg("WC_SKARBIEC_CONSUMER", "secrets.skarbiec.consumer", "stado"));
static SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    expand_tilde(&cfg(
        "WC_SKARBIEC_TOKEN_FILE",
        "secrets.skarbiec.token_file",
        "~/.stado/stado-skarbiec-token",
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

/// The address of the Skarbiec this machine uses; empty when none resolves,
/// and then [`skarbiec_url_absent`] says why.
pub fn skarbiec_url() -> &'static str {
    match SKARBIEC_URL.as_ref() {
        Ok(url) => url,
        // The cause stays in the static and `skarbiec_url_absent` hands
        // it on; the empty address is what every caller tests for.
        Err(_explained_by_skarbiec_url_absent) => "",
    }
}

/// Why this machine has no Skarbiec address: nothing names one, or the
/// forward marker that would cannot be read. `None` while one resolves.
pub fn skarbiec_url_absent() -> Option<&'static str> {
    SKARBIEC_URL.as_ref().err().map(String::as_str)
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
