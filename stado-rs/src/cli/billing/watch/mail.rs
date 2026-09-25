//! The advisory mail sweep. Providers announce closure, failed payment and
//! credit expiry by email days before the API starts refusing calls, so the
//! watchdog reads what Skrzynka has received beside every poll — and never
//! fails because it could not.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::constants::{MESSAGES_READ, NEWER_THAN_DAYS, SENDER_DOMAINS};
use crate::mail::{self, MailAnalysis, MailAnalysisReport, SkrzynkaMessage};

/// What this sweep reads, in words: the report's `query`.
fn scope() -> String {
    format!(
        "skrzynka: mail from {} received in the last {NEWER_THAN_DAYS} days",
        SENDER_DOMAINS.join(", ")
    )
}

/// The address part of a sender such as `Azure <no-reply@azure.microsoft.com>`.
fn sender_domain(sender: &str) -> Option<String> {
    let address = sender
        .rsplit_once('<')
        .map_or(sender, |(_, rest)| rest.trim_end_matches('>'));
    address
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim().to_ascii_lowercase())
}

fn is_provider_notice(message: &SkrzynkaMessage, now: DateTime<Utc>) -> bool {
    let recent = DateTime::parse_from_rfc3339(&message.received_at).is_ok_and(|received| {
        now.signed_duration_since(received.with_timezone(&Utc))
            <= chrono::Duration::days(NEWER_THAN_DAYS)
    });
    let from_provider = sender_domain(&message.sender).is_some_and(|domain| {
        SENDER_DOMAINS
            .iter()
            .any(|declared| domain == *declared || domain.ends_with(&format!(".{declared}")))
    });
    recent && from_provider
}

/// Outcome of the advisory mail sweep. `Unavailable` is a first-class,
/// non-fatal state: the watchdog is expected to run on boxes where Skrzynka
/// is not installed or holds no mailbox at all.
pub(super) enum MailProbe {
    Report(Box<MailAnalysisReport>),
    Unavailable(String),
}

impl MailProbe {
    pub(super) fn as_value(&self) -> Value {
        match self {
            Self::Report(report) => json!({
                "status": "ok",
                "query": report.query,
                "message_count": report.message_count,
                "action_required_count": report.action_required_count,
                "messages": report.messages,
            }),
            Self::Unavailable(detail) => json!({
                "status": "unavailable",
                "query": scope(),
                "detail": detail,
            }),
        }
    }

    pub(super) fn summary(&self) -> String {
        match self {
            Self::Report(report) => format!(
                "{} msg / {} action",
                report.message_count, report.action_required_count
            ),
            Self::Unavailable(_) => "unavailable".to_string(),
        }
    }

    /// The messages a human has to act on, in the order Skrzynka listed
    /// them. These are what get printed under a degraded provider.
    pub(super) fn action_required(&self) -> Vec<&MailAnalysis> {
        match self {
            Self::Report(report) => report
                .messages
                .iter()
                .filter(|message| message.action_required)
                .collect(),
            Self::Unavailable(_) => Vec::new(),
        }
    }
}

/// Read provider notices from Skrzynka. Every failure path — Skrzynka not
/// installed, refusing, or answering something unreadable — degrades to
/// [`MailProbe::Unavailable`] carrying the exact cause. Nothing here can
/// return an error, by construction: a watchdog that stops watching because
/// its mailbox is unreachable is the failure mode this whole command exists
/// to eliminate.
pub(super) async fn mail_probe() -> MailProbe {
    let received = match mail::messages(MESSAGES_READ).await {
        Ok(received) => received,
        Err(err) => return MailProbe::Unavailable(err.to_string()),
    };
    let now = Utc::now();
    let notices = received
        .iter()
        .filter(|message| is_provider_notice(message, now))
        .map(mail::analyze)
        .collect();
    MailProbe::Report(Box::new(mail::summarize(&scope(), notices)))
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::is_provider_notice;
    use crate::mail::SkrzynkaMessage;

    fn received(sender: &str, received_at: &str) -> SkrzynkaMessage {
        SkrzynkaMessage {
            id: "message".to_string(),
            mailbox_id: "mailbox".to_string(),
            sender: sender.to_string(),
            recipients: "ops@example.com".to_string(),
            subject: "Your invoice".to_string(),
            sent_at: None,
            received_at: received_at.to_string(),
            body_text: "USD 1,204.55 is due".to_string(),
            snippet: String::new(),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-25T00:00:00Z")
            .expect("fixed instant")
            .with_timezone(&Utc)
    }

    #[test]
    fn a_recent_message_from_a_provider_subdomain_is_a_notice() {
        let message = received(
            "Microsoft Azure <azure-noreply@azure.microsoft.com>",
            "2026-09-20T08:00:00Z",
        );
        assert!(is_provider_notice(&message, now()));
    }

    #[test]
    fn a_lookalike_domain_is_not_a_provider() {
        let message = received("billing@notmicrosoft.com", "2026-09-20T08:00:00Z");
        assert!(!is_provider_notice(&message, now()));
    }

    #[test]
    fn a_notice_older_than_the_window_is_not_read() {
        let message = received("payments-noreply@google.com", "2026-09-01T08:00:00Z");
        assert!(!is_provider_notice(&message, now()));
    }
}
