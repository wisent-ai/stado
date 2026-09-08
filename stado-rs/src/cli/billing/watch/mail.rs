//! The advisory mail sweep. Providers announce closure, failed payment and
//! credit expiry by email days before the API starts refusing calls, so the
//! watchdog reads that mailbox beside every poll — and never fails because
//! it could not.

use serde_json::{json, Value};

use crate::mail::{self, GmailClient, MailAnalysis, MailAnalysisReport};

/// Gmail expression for provider billing notices. Deliberately narrow on
/// sender and broad on wording: the subject lines differ per provider and
/// per notice type, but the sender domains do not. The `cli/mail.rs` help
/// example is the Microsoft/Azure half of exactly this query.
const MAIL_QUERY: &str = "newer_than:14d (from:microsoft.com OR from:azure.microsoft.com \
     OR from:google.com OR from:googlecloud.com OR from:payments-noreply.google.com) \
     (billing OR invoice OR payment OR subscription OR credit OR suspended OR \"past due\")";

/// Outcome of the advisory mail sweep. `Unavailable` is a first-class,
/// non-fatal state: the watchdog is expected to run on boxes with no Gmail
/// credentials at all.
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
                "query": MAIL_QUERY,
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

    /// The messages a human has to act on, newest first as Gmail returned
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

/// Sweep Gmail for provider billing notices. Every failure path — missing
/// token, missing scope, Gmail unreachable — degrades to
/// [`MailProbe::Unavailable`] carrying the exact cause. Nothing here can
/// return an error, by construction: a watchdog that stops watching because
/// its mailbox is unreachable is the failure mode this whole command exists
/// to eliminate.
pub(super) async fn mail_probe() -> MailProbe {
    let client = match GmailClient::from_env().await {
        Ok(client) => client,
        Err(err) => return MailProbe::Unavailable(err.to_string()),
    };
    match client
        .analyze(MAIL_QUERY, crate::cli::default_mail_results())
        .await
    {
        Ok(messages) => MailProbe::Report(Box::new(mail::summarize(MAIL_QUERY, messages))),
        Err(err) => MailProbe::Unavailable(err.to_string()),
    }
}
