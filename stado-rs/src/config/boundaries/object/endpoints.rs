//! Skarbiec endpoint of the least-privilege object-token verifier.

use std::sync::LazyLock;

use super::OBJECT_API_VERIFIER_CONSUMER;
use crate::config::skarbiec_url;
use crate::config_file::{expand_tilde, resolve as cfg};

static OBJECT_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_OBJECT_SKARBIEC_URL",
        "object_api.skarbiec.url",
        skarbiec_url(),
    )
});
static OBJECT_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_OBJECT_SKARBIEC_CONSUMER",
        "object_api.skarbiec.consumer",
        OBJECT_API_VERIFIER_CONSUMER,
    )
});
static OBJECT_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-object-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_OBJECT_SKARBIEC_TOKEN_FILE",
        "object_api.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

/// Skarbiec endpoint used only by the product-token verifier grant.
pub fn object_skarbiec_url() -> &'static str {
    OBJECT_SKARBIEC_URL.as_str()
}

/// Dedicated least-privilege consumer that can read exactly the mapped items.
pub fn object_skarbiec_consumer() -> &'static str {
    OBJECT_SKARBIEC_CONSUMER.as_str()
}

/// Owner-only grant file for the dedicated object-token verifier.
pub fn object_skarbiec_token_file() -> &'static str {
    OBJECT_SKARBIEC_TOKEN_FILE.as_str()
}
