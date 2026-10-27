//! Channel resolution. [`AlertChannels::from_env`] reads the enablement
//! fence, asks the selected credential store (Skarbiec, or the owner-only
//! file store on a machine without it) for exactly the fields the enabled
//! channels need, and hands every gap to `channel_failed` instead of a
//! panic. The resolvers with a story of their own — Skrzynka's mailbox
//! declaration and most's Twilio material — sit beside it, together with the
//! Pub/Sub OAuth fetch.

use super::{channel_failed, AlertChannels, PubSubChannel, TelegramChannel, PUBSUB_BASE, TELEGRAM_API_BASE};

mod providers;
use providers::{gcp_token, resolve_most, resolve_skrzynka};

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

        let needs_stored = is_enabled("slack") || is_enabled("telegram") || is_enabled("most");
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
            if is_enabled("most") {
                wanted.push("most_phone");
            }
            // Through the selected credential store: Skarbiec with Stado's
            // configured grant, or the owner-only file store a machine without
            // Skarbiec selects with STADO_CREDENTIALS_STORE.
            let mut found = std::collections::BTreeMap::new();
            for field in wanted {
                match crate::credential_store::read_string("alerts", field).await {
                    Ok(Some(value)) if !value.is_empty() => {
                        found.insert(field.to_string(), value);
                    }
                    Ok(_) => {}
                    Err(err) => channel_failed("configuration", &err.to_string()),
                }
            }
            found
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
        let skrzynka = if is_enabled("skrzynka") {
            resolve_skrzynka()
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
            skrzynka,
            pubsub,
            most,
        }
    }
}
