//! Alert destinations. Stado reads their keys through its Skarbiec identity.

use std::sync::LazyLock;

use crate::config::project;
use crate::config_file::resolve as cfg;

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

/// Where email alerts go. The destination is not a secret, so it belongs in
/// the config document rather than the vault; the env name matches the one
/// the SendGrid channel has always read.
static ALERT_EMAIL_TO: LazyLock<String> =
    LazyLock::new(|| cfg("WC_EMAIL_TO", "alerts.email_to", ""));

/// The Skrzynka mailbox email alerts are sent from. Skrzynka holds that
/// account's credentials and does the sending; Stado names only the mailbox.
static ALERT_SKRZYNKA_MAILBOX: LazyLock<String> =
    LazyLock::new(|| cfg("WC_ALERT_SKRZYNKA_MAILBOX", "alerts.skrzynka_mailbox", ""));

/// The Skrzynka organization that holds [`alert_skrzynka_mailbox`].
static ALERT_SKRZYNKA_ORGANIZATION: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_ALERT_SKRZYNKA_ORGANIZATION",
        "alerts.skrzynka_organization",
        "",
    )
});

/// Pub/Sub alerts topic (env `WC_ALERTS_TOPIC`).
pub fn alerts_topic() -> &'static str {
    ALERTS_TOPIC.as_str()
}

/// Destination for email alert channels (env `WC_EMAIL_TO`).
pub fn alert_email_to() -> &'static str {
    ALERT_EMAIL_TO.as_str()
}

/// Mailbox the skrzynka channel sends from (env `WC_ALERT_SKRZYNKA_MAILBOX`).
pub fn alert_skrzynka_mailbox() -> &'static str {
    ALERT_SKRZYNKA_MAILBOX.as_str()
}

/// Organization holding that mailbox (env `WC_ALERT_SKRZYNKA_ORGANIZATION`).
pub fn alert_skrzynka_organization() -> &'static str {
    ALERT_SKRZYNKA_ORGANIZATION.as_str()
}
