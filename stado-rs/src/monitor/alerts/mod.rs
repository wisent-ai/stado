//! Optional Slack, Telegram, SendGrid, Resend, most (SMS), and GCP Pub/Sub
//! alert delivery. `alerts.channels` is the explicit enablement fence: with no
//! enabled channels, dispatch performs no credential or network lookup, and
//! each delivery is fault-isolated with a bounded structured failure line.
//!
//! The components are the seams this account already had: `channels` holds
//! the resolved per-target structs, `resolve` the enablement read and the
//! Skarbiec lookups that fill them, `dispatch` the fan-out over a resolved
//! set, and `send` the HTTP request each channel needs. The vocabulary all
//! four share — the endpoint bases, the sender default, the `[alert]` sink
//! and the failure line that keeps one dead channel from stopping the rest —
//! stays here. Every name a caller outside this module uses is re-exported
//! here, so `crate::monitor::alerts::<item>` resolves exactly as before.

mod channels;
mod dispatch;
mod resolve;
mod send;

pub use channels::{
    AlertChannels, MostChannel, PubSubChannel, ResendChannel, SendgridChannel, TelegramChannel,
};
pub use dispatch::{send_alert, send_alert_with};
pub(crate) use send::resend_verified_domains;

/// GCP OAuth scope for the Pub/Sub publish call.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
/// Telegram Bot API base (overridable per-channel for tests).
const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
/// SendGrid mail-send endpoint.
const SENDGRID_URL: &str = "https://api.sendgrid.com/v3/mail/send";
/// Resend mail-send endpoint.
const RESEND_URL: &str = "https://api.resend.com/emails";
/// Pub/Sub REST base.
const PUBSUB_BASE: &str = "https://pubsub.googleapis.com";
/// Twilio REST base for the most (SMS) channel.
const TWILIO_API_BASE: &str = "https://api.twilio.com";
/// Python `WC_EMAIL_FROM` default.
const DEFAULT_EMAIL_FROM: &str = "compute@example.com";

fn log(msg: &str) {
    eprintln!("[alert] {msg}");
}

/// One channel could not deliver. Logged twice on purpose and fatal never:
/// the `[alert]` line is what a human tailing the monitor reads, and the
/// structured line is what a log query finds a week later.
fn channel_failed(channel: &str, error: &str) {
    let code = crate::failure::classify_message(error);
    tracing::error!(
        failure_point = "monitor.alerts.deliver",
        error_code = code.as_str(),
        service = "alerts",
        retryable = code.retryable(),
        severity = code.severity().as_str(),
        channel = channel,
        detail = %crate::failure::bounded_detail(error),
        "alert channel delivery failed; the remaining channels still fire"
    );
    log(&format!("{channel} failed: {error}"));
}
