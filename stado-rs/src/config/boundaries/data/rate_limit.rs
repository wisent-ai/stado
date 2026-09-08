//! Rate-limit boundary verifier consumer and its Skarbiec grant.

use std::sync::LazyLock;

use crate::config::skarbiec_url;
use crate::config_file::{expand_tilde, resolve as cfg};

pub const RATE_LIMIT_API_VERIFIER_CONSUMER: &str = "stado-rate-limit-api-verifier";

static RATE_LIMIT_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RATE_LIMIT_SKARBIEC_URL",
        "rate_limit.skarbiec.url",
        skarbiec_url(),
    )
});
static RATE_LIMIT_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_RATE_LIMIT_SKARBIEC_CONSUMER",
        "rate_limit.skarbiec.consumer",
        RATE_LIMIT_API_VERIFIER_CONSUMER,
    )
});
static RATE_LIMIT_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-rate-limit-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_RATE_LIMIT_SKARBIEC_TOKEN_FILE",
        "rate_limit.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

pub fn rate_limit_skarbiec_url() -> &'static str {
    RATE_LIMIT_SKARBIEC_URL.as_str()
}

pub fn rate_limit_skarbiec_consumer() -> &'static str {
    RATE_LIMIT_SKARBIEC_CONSUMER.as_str()
}

pub fn rate_limit_skarbiec_token_file() -> &'static str {
    RATE_LIMIT_SKARBIEC_TOKEN_FILE.as_str()
}
