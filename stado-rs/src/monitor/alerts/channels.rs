//! The resolved channel structs: one per delivery target, plus the
//! [`AlertChannels`] set that holds them. Data only — the reads that fill
//! them live in `resolve` and the requests that drain them in `send`.

/// Telegram channel config (Skarbiec bot token + configured chat id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramChannel {
    pub token: String,
    pub chat_id: String,
    /// Bot API base URL; the request path is `/bot{token}/sendMessage`.
    pub api_base: String,
}

/// SendGrid channel config (`sendgrid_api_key` from Skarbiec plus non-secret
/// recipient/sender configuration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendgridChannel {
    pub api_key: String,
    pub to: String,
    pub from: String,
    pub url: String,
}

/// Resend channel config. The key is this deployment's own `RESEND_API_KEY`
/// vault item rather than a copy inside `stado-alerts`: one secret, one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResendChannel {
    pub api_key: String,
    pub to: String,
    pub from: String,
    pub url: String,
}

/// Pub/Sub channel config: full `projects/{p}/topics/{t}` topic path plus a
/// pre-fetched OAuth token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubSubChannel {
    pub topic: String,
    pub base_url: String,
    pub token: String,
}

/// most (SMS) channel: destination from `stado-alerts/most_phone`, Twilio
/// credentials resolved from `most-twilio` through the `most` integration
/// provider grant, delivered in-process so the alert path never depends on
/// the dashboard it may be alerting about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MostChannel {
    pub phone: String,
    pub account_sid: String,
    pub auth_token: String,
    pub api_version: String,
    pub messaging_service_sid: Option<String>,
    pub from_number: Option<String>,
    /// Twilio REST base; tests point it at the loopback mock.
    pub api_base: String,
}

/// Resolved alert-channel configuration; channels with no config are skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlertChannels {
    /// Slack webhook URL from `stado-alerts/slack_webhook` in Skarbiec.
    pub slack_webhook: Option<String>,
    pub telegram: Option<TelegramChannel>,
    pub sendgrid: Option<SendgridChannel>,
    pub resend: Option<ResendChannel>,
    pub pubsub: Option<PubSubChannel>,
    pub most: Option<MostChannel>,
}
