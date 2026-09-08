//! At least one alert channel that survives the cloud going away.

use crate::config;
use crate::doctor::{provider_enabled, storage_adapter, Check, Findings, Status};
use crate::monitor::alerts::AlertChannels;

// ---------------------------------------------------------------------------
// 10. Alerts
// ---------------------------------------------------------------------------

pub(in crate::doctor) const ALERTS_ID: &str = "alerts";
pub(in crate::doctor) const ALERTS_TITLE: &str = "Alerts";
pub(in crate::doctor) const ALERTS_REMEDY: &str =
    "configure at least one non-GCP channel: enable it in alerts.channels and give it its \
     material - slack_webhook, telegram_bot_token + telegram_chat_id, or sendgrid_api_key in \
     the stado-alerts Skarbiec item, or resend with email_to there and the RESEND_API_KEY \
     item; clear WC_ALERTS_TOPIC on a deployment that has left GCP";

/// At least one alert channel that survives the cloud going away.
///
/// The outage's compounding failure: the only configured channel was GCP
/// Pub/Sub, on the very account whose billing had been disabled, so every
/// alert about the outage failed to send because of the outage.
pub(in crate::doctor) async fn check_alerts() -> Check {
    // An empty topic short-circuits the Pub/Sub arm of `from_env`, so this
    // resolves the three non-GCP channels through the production logic
    // without paying for (or logging) a GCP token probe.
    let channels = AlertChannels::from_env("").await;
    let mut configured: Vec<&str> = Vec::new();
    if channels.slack_webhook.is_some() {
        configured.push("slack");
    }
    if channels.telegram.is_some() {
        configured.push("telegram");
    }
    if channels.sendgrid.is_some() {
        configured.push("sendgrid");
    }
    if channels.resend.is_some() {
        configured.push("resend");
    }
    if channels.most.is_some() {
        configured.push("most");
    }

    // A resolved channel is not a working one. The provider is the only
    // authority on whether this key is still valid and whether it may send as
    // this sender, and asking costs one read: the deployment sat green for
    // weeks holding a key Resend had already revoked.
    // What the operator declared, held apart from what resolved here.
    //
    // A vault that did not answer this second is not a deployment with no
    // alerts. On 2026-09-02 at 19:31:04 this check FAILED a release delivery
    // with "no alert channel is configured at all" while `alerts.channels`
    // held `resend` and `alerts.email_to` its destination; the same check
    // PASSED two minutes later against the same file, because that time the
    // material read succeeded. Instance 9's shape, inside the preflight that
    // gates delivery: absent and unreachable need opposite responses.
    let declared: Vec<&str> = config::alert_channels()
        .iter()
        .map(String::as_str)
        .collect();
    let resolved_any = !configured.is_empty();

    let resend_problem: Option<(Status, String)> = match &channels.resend {
        Some(resend) => {
            let client = reqwest::Client::new();
            match crate::monitor::alerts::resend_verified_domains(&client, resend).await {
                Ok(domains) => {
                    let sender_domain = resend.from.rsplit('@').next().unwrap_or_default();
                    if domains.iter().any(|domain| domain == sender_domain) {
                        None
                    } else {
                        configured.retain(|channel| *channel != "resend");
                        Some((
                            Status::Fail,
                            format!(
                                "resend sender {} is not on a verified domain; verified: [{}]",
                                resend.from,
                                domains.join(",")
                            ),
                        ))
                    }
                }
                // The provider answering "no" and this host being unable to
                // ask are different facts with different owners. `HTTP <code>`
                // is the provider's own verdict; anything else is a transport
                // this deployment can retry, and failing a delivery on it
                // pages nobody about a channel that works.
                Err(error) => {
                    configured.retain(|channel| *channel != "resend");
                    if error.starts_with("HTTP ") {
                        Some((
                            Status::Fail,
                            format!("resend key was refused by the provider: {error}"),
                        ))
                    } else {
                        Some((
                            Status::Warn,
                            format!("resend could not be asked from this host: {error}"),
                        ))
                    }
                }
            }
        }
        None => None,
    };

    let topic = config::alerts_topic();
    // "On GCP" means there is still a GCP surface a Pub/Sub publish could
    // plausibly authenticate against: the GCS queue store or the GCP
    // dispatch provider. The billing outage removed both at once.
    let on_gcp = storage_adapter(config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::Gcs)
        || provider_enabled(crate::capabilities::ProviderId::Gcp);
    let mut findings = Findings::default();

    if !configured.is_empty() {
        findings.note(
            Status::Pass,
            format!("non-GCP channel(s) configured: {}", configured.join(",")),
        );
    } else if resolved_any {
        // Something resolved and the checks below disqualified it. Their own
        // sentences carry the verdict; this line must not claim the operator
        // configured nothing.
        findings.note(
            Status::Warn,
            "every channel that resolved was disqualified by the finding(s) below".to_string(),
        );
    } else if !declared.is_empty() {
        findings.note(
            Status::Warn,
            format!(
                "alerts.channels declares [{}] and none of them resolved their material on this \
                 host; that is an unreadable channel, not an unconfigured one — \
                 `stado alerts channels` names the read that failed",
                declared.join(",")
            ),
        );
        findings.remedy(
            "read the failing channel's material with `stado alerts channels`; a vault or broker \
             that did not answer is the thing to fix, and the declaration in alerts.channels is \
             already correct",
        );
    } else {
        let detail = if topic.is_empty() {
            "no alert channel is configured at all; nothing anywhere will page an operator"
                .to_string()
        } else if on_gcp {
            format!(
                "the only channel is GCP Pub/Sub ({topic}); an outage of that account takes the \
                 alerts down with it, which is exactly how the last one went unnoticed"
            )
        } else {
            format!(
                "the only channel is GCP Pub/Sub ({topic}) but this deployment has no GCP \
                 surface left (backend={}, providers=[{}]); every alert is delivered nowhere",
                config::wc_storage_backend(),
                config::wc_providers().join(",")
            )
        };
        let status = if topic.is_empty() || !on_gcp {
            Status::Fail
        } else {
            Status::Warn
        };
        findings.note(status, detail);
        findings.remedy(ALERTS_REMEDY);
    }

    if let Some((status, problem)) = resend_problem {
        findings.note(status, problem);
        findings.remedy(
            "point alerts.resend_item at an item holding a key the provider accepts, and \
             alerts.email_from at a verified sending domain; `stado alerts channels` shows \
             what resolved and `stado alerts send` proves delivery",
        );
    }

    if !topic.is_empty() && !on_gcp {
        findings.note(
            Status::Warn,
            format!(
                "WC_ALERTS_TOPIC is set to {topic} on a deployment with no GCP surface; every \
                 send_alert pays a failing gcp_auth probe before the working channels fire"
            ),
        );
        findings.remedy("unset WC_ALERTS_TOPIC (config key alerts.topic) on this deployment");
    }

    findings.into_check(ALERTS_ID, ALERTS_TITLE, ALERTS_REMEDY)
}
