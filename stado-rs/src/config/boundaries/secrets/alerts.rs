//! Alert destinations. Stado reads their keys through its Skarbiec identity.

use std::sync::LazyLock;

use crate::config::project;
use crate::config_file::resolve as cfg;

/// The Pub/Sub topic alerts go to: `WC_ALERTS_TOPIC` when set and not
/// blank, else `alerts.topic` from the config file, else the project's
/// topic named after this product when a project is declared; empty when
/// none is.
static ALERTS_TOPIC: LazyLock<String> = LazyLock::new(|| {
    let from_file_or_project = || {
        let default = if project().is_empty() {
            String::new()
        } else {
            format!(
                "projects/{}/topics/{}-alerts",
                project(),
                env!("CARGO_PKG_NAME")
            )
        };
        cfg("", "alerts.topic", &default)
    };
    match std::env::var("WC_ALERTS_TOPIC") {
        Ok(topic) if !topic.is_empty() => topic,
        Ok(_) | Err(std::env::VarError::NotPresent) => from_file_or_project(),
        // A topic name that is not Unicode is not a topic: the file and the
        // project decide, and `config validate` names the variable.
        Err(std::env::VarError::NotUnicode(_raw)) => from_file_or_project(),
    }
});

/// Where email alerts go. The destination is not a secret, so it belongs in
/// the config document rather than the vault; the Skrzynka channel sends to it.
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
