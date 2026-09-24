//! Alert destinations. Stado reads their keys through its Skarbiec identity.

use std::sync::LazyLock;

use crate::config::project;
use crate::config_file::{resolve as cfg, resolve_list as cfg_list};

static ALERTS_TOPIC: LazyLock<String> = LazyLock::new(|| {
    std::env::var("WC_ALERTS_TOPIC")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            let default = if project().is_empty() {
                String::new()
            } else {
                format!("projects/{}/topics/stado-alerts", project())
            };
            cfg("", "alerts.topic", &default)
        })
});

static ALERT_CHANNELS: LazyLock<Vec<String>> =
    LazyLock::new(|| cfg_list("STADO_ALERT_CHANNELS", "alerts.channels", &[]));

/// Where email alerts go. The destination is not a secret, so it belongs in
/// the config document rather than the vault; the env name matches the one
/// the SendGrid channel has always read.
static ALERT_EMAIL_TO: LazyLock<String> =
    LazyLock::new(|| cfg("WC_EMAIL_TO", "alerts.email_to", ""));

/// Sender for email alerts; must be a domain the provider has verified.
static ALERT_EMAIL_FROM: LazyLock<String> =
    LazyLock::new(|| cfg("WC_EMAIL_FROM", "alerts.email_from", ""));

/// Vault item holding the Resend API key, and the field inside it. A
/// deployment's live key is not always in the item a default would guess:
/// this one keeps it in the Weles management item, while `RESEND_API_KEY`
/// holds a key the provider has already rejected.
static ALERT_RESEND_ITEM: LazyLock<String> =
    LazyLock::new(|| cfg("WC_RESEND_ITEM", "alerts.resend_item", "RESEND_API_KEY"));

static ALERT_RESEND_FIELD: LazyLock<String> =
    LazyLock::new(|| cfg("WC_RESEND_FIELD", "alerts.resend_field", "value"));

/// Pub/Sub alerts topic (env `WC_ALERTS_TOPIC`).
pub fn alerts_topic() -> &'static str {
    ALERTS_TOPIC.as_str()
}

/// Explicitly enabled optional alert adapters.
pub fn alert_channels() -> &'static [String] {
    ALERT_CHANNELS.as_slice()
}

/// Destination for email alert channels (env `WC_EMAIL_TO`).
pub fn alert_email_to() -> &'static str {
    ALERT_EMAIL_TO.as_str()
}

/// Sender for email alert channels (env `WC_EMAIL_FROM`).
pub fn alert_email_from() -> &'static str {
    ALERT_EMAIL_FROM.as_str()
}

/// Vault item holding the Resend API key (env `WC_RESEND_ITEM`).
pub fn alert_resend_item() -> &'static str {
    ALERT_RESEND_ITEM.as_str()
}

/// Field inside [`alert_resend_item`] (env `WC_RESEND_FIELD`).
pub fn alert_resend_field() -> &'static str {
    ALERT_RESEND_FIELD.as_str()
}
