//! Skarbiec endpoints of the integration client and provider grants.

use std::sync::LazyLock;

use super::INTEGRATION_API_VERIFIER_CONSUMER;
use crate::config::skarbiec_url;
use crate::config_file::{expand_tilde, resolve as cfg};

static INTEGRATION_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_INTEGRATION_SKARBIEC_URL",
        "integration.skarbiec.url",
        skarbiec_url(),
    )
});
static INTEGRATION_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_INTEGRATION_SKARBIEC_CONSUMER",
        "integration.skarbiec.consumer",
        INTEGRATION_API_VERIFIER_CONSUMER,
    )
});
static INTEGRATION_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-integration-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_INTEGRATION_SKARBIEC_TOKEN_FILE",
        "integration.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});
static INTEGRATION_PROVIDER_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_INTEGRATION_PROVIDER_SKARBIEC_URL",
        "integration.provider_skarbiec.url",
        skarbiec_url(),
    )
});

pub fn integration_skarbiec_url() -> &'static str {
    INTEGRATION_SKARBIEC_URL.as_str()
}

pub fn integration_skarbiec_consumer() -> &'static str {
    INTEGRATION_SKARBIEC_CONSUMER.as_str()
}

pub fn integration_skarbiec_token_file() -> &'static str {
    INTEGRATION_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn integration_provider_skarbiec_url() -> &'static str {
    INTEGRATION_PROVIDER_SKARBIEC_URL.as_str()
}
