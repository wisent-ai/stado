//! Channel resolution. [`AlertChannels::from_env`] reads the enablement
//! fence, asks Skarbiec for exactly the fields the enabled channels need,
//! and hands every gap to `channel_failed` instead of a panic. The two
//! resolvers with a story of their own — Resend's config-or-vault
//! destination and most's provider-grant Twilio material — sit beside it,
//! together with the Pub/Sub OAuth fetch.

use super::{
    channel_failed, AlertChannels, PubSubChannel, SendgridChannel, TelegramChannel,
    DEFAULT_EMAIL_FROM, PUBSUB_BASE, SENDGRID_URL, TELEGRAM_API_BASE,
};

mod providers;
use providers::{gcp_token, resolve_most, resolve_resend};

impl AlertChannels {
    /// Resolve the channels the operator chose (`operator_contact` in the
    /// registry, see [`super::contact`]). With no choice recorded, or one the
    /// registry cannot answer, nothing resolves and the cause is reported.
    pub async fn from_env(topic: &str) -> Self {
        let enabled = match super::contact::chosen().await {
            Ok(chosen) => chosen,
            Err(cause) => {
                channel_failed("operator-contact", &cause);
                return Self::default();
            }
        };
        let is_enabled = |channel: &str| enabled.iter().any(|value| value == channel);

        let needs_stored = is_enabled("slack")
            || is_enabled("telegram")
            || is_enabled("sendgrid")
            || is_enabled("resend")
            || is_enabled("most");
        let stored: std::collections::BTreeMap<String, String> = if needs_stored {
            // Per field, not the whole item: the listener refuses a read that
            // names no field ("field required"), so asking for the item as a
            // whole silently emptied every channel and the operator was left
            // with an alerting path that resolved to nothing.
            let mut wanted: Vec<&str> = Vec::new();
            if is_enabled("slack") {
                wanted.push("slack_webhook");
            }
            if is_enabled("telegram") {
                wanted.push("telegram_bot_token");
                wanted.push("telegram_chat_id");
            }
            if is_enabled("sendgrid") {
                wanted.push("sendgrid_api_key");
            }
            if is_enabled("resend") {
                // Only ask the vault for what the config document does not
                // already answer: a deployment that names its destination in
                // config holds no such field, and the read would be refused.
                if crate::config::alert_email_to().is_empty() {
                    wanted.push("email_to");
                }
                if crate::config::alert_email_from().is_empty() {
                    wanted.push("email_from");
                }
            }
            if is_enabled("most") {
                wanted.push("most_phone");
            }
            match crate::skarbiec::Client::configured() {
                Ok(vault) => {
                    let mut found = std::collections::BTreeMap::new();
                    for field in wanted {
                        match vault.read_string("alerts", field).await {
                            Ok(Some(value)) if !value.is_empty() => {
                                found.insert(field.to_string(), value);
                            }
                            Ok(_) => {}
                            Err(err) => channel_failed("configuration", &err.to_string()),
                        }
                    }
                    found
                }
                Err(err) => {
                    channel_failed("configuration", &err.to_string());
                    std::collections::BTreeMap::new()
                }
            }
        } else {
            std::collections::BTreeMap::new()
        };
        let secret = |field: &str| stored.get(field).cloned();
        // An enabled channel that resolves to nothing says why. Without it a
        // worker host refused every page with "no alert channel resolves;
        // enabled: [most]" and nothing named the field that was missing.
        let absent = |channel: &str, fields: &[&str]| {
            channel_failed(
                &format!("{channel}-configuration"),
                &format!(
                    "the vault's alerts item holds no {}; the operator chose the {channel} channel \
                     (operator_contact) and it cannot page without it",
                    fields.join(" or ")
                ),
            );
        };

        let slack_webhook = if is_enabled("slack") {
            let webhook = secret("slack_webhook");
            if webhook.is_none() {
                absent("slack", &["slack_webhook"]);
            }
            webhook
        } else {
            None
        };
        let telegram = if is_enabled("telegram") {
            match (secret("telegram_bot_token"), secret("telegram_chat_id")) {
                (Some(token), Some(chat_id)) => Some(TelegramChannel {
                    token,
                    chat_id,
                    api_base: TELEGRAM_API_BASE.to_string(),
                }),
                _ => {
                    absent("telegram", &["telegram_bot_token", "telegram_chat_id"]);
                    None
                }
            }
        } else {
            None
        };
        let sendgrid = if is_enabled("sendgrid") {
            match (
                secret("sendgrid_api_key"),
                std::env::var("WC_EMAIL_TO").ok(),
            ) {
                (Some(api_key), Some(to)) if !to.is_empty() => Some(SendgridChannel {
                    api_key,
                    to,
                    from: std::env::var("WC_EMAIL_FROM")
                        .ok()
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| DEFAULT_EMAIL_FROM.to_string()),
                    url: SENDGRID_URL.to_string(),
                }),
                (None, _) => {
                    absent("sendgrid", &["sendgrid_api_key"]);
                    None
                }
                _ => {
                    channel_failed(
                        "sendgrid-configuration",
                        "no destination: set WC_EMAIL_TO for the sendgrid channel",
                    );
                    None
                }
            }
        } else {
            None
        };
        let resend = if is_enabled("resend") {
            resolve_resend(secret("email_to"), secret("email_from")).await
        } else {
            None
        };
        let most = if is_enabled("most") {
            let phone = secret("most_phone");
            if phone.is_none() {
                absent("most", &["most_phone"]);
            }
            resolve_most(phone).await
        } else {
            None
        };
        if is_enabled("gcp-pubsub") && topic.is_empty() {
            channel_failed(
                "pubsub-configuration",
                "the gcp-pubsub channel is enabled but alerts.topic (WC_ALERTS_TOPIC) names no topic",
            );
        }
        let pubsub = if is_enabled("gcp-pubsub") && !topic.is_empty() {
            match gcp_token().await {
                Ok(token) => Some(PubSubChannel {
                    topic: topic.to_string(),
                    base_url: PUBSUB_BASE.to_string(),
                    token,
                }),
                Err(err) => {
                    channel_failed("pubsub-auth", &err);
                    None
                }
            }
        } else {
            None
        };

        Self {
            slack_webhook,
            telegram,
            sendgrid,
            resend,
            pubsub,
            most,
        }
    }
}
