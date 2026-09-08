//! Skarbiec endpoints of the release-serving and release-signing grants.

use std::sync::LazyLock;

use super::{RELEASE_API_VERIFIER_CONSUMER, RELEASE_SIGNING_CONSUMER};
use crate::config::skarbiec_url;
use crate::config_file::{expand_tilde, resolve as cfg};

static RELEASE_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RELEASE_SKARBIEC_URL",
        "release_api.skarbiec.url",
        skarbiec_url(),
    )
});
static RELEASE_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RELEASE_SKARBIEC_CONSUMER",
        "release_api.skarbiec.consumer",
        RELEASE_API_VERIFIER_CONSUMER,
    )
});
static RELEASE_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-release-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_RELEASE_SKARBIEC_TOKEN_FILE",
        "release_api.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

static RELEASE_SIGNING_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RELEASE_SIGNING_SKARBIEC_CONSUMER",
        "release.signing_skarbiec.consumer",
        RELEASE_SIGNING_CONSUMER,
    )
});
static RELEASE_SIGNING_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-release-coordinator-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_RELEASE_SIGNING_SKARBIEC_TOKEN_FILE",
        "release.signing_skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

pub fn release_skarbiec_url() -> &'static str {
    RELEASE_SKARBIEC_URL.as_str()
}

pub fn release_skarbiec_consumer() -> &'static str {
    RELEASE_SKARBIEC_CONSUMER.as_str()
}

pub fn release_skarbiec_token_file() -> &'static str {
    RELEASE_SKARBIEC_TOKEN_FILE.as_str()
}

pub fn release_signing_skarbiec_consumer() -> &'static str {
    RELEASE_SIGNING_SKARBIEC_CONSUMER.as_str()
}

pub fn release_signing_skarbiec_token_file() -> &'static str {
    RELEASE_SIGNING_SKARBIEC_TOKEN_FILE.as_str()
}
