//! The channel resolvers with a story of their own — Skrzynka's mailbox
//! declaration and most's provider-grant Twilio material — and the Pub/Sub
//! OAuth fetch.

use super::super::{
    channel_failed, MostChannel, SkrzynkaChannel, CLOUD_PLATFORM_SCOPE, TWILIO_API_BASE,
};

/// Resolve the Skrzynka email channel from the config document: the
/// destination `alerts.email_to`, and the mailbox Skrzynka sends from,
/// `alerts.skrzynka_mailbox` held under `alerts.skrzynka_organization`. No
/// credential is read here: Skrzynka holds the mailbox's own. Every missing
/// declaration is named in one structured failure line.
pub(super) fn resolve_skrzynka() -> Option<SkrzynkaChannel> {
    let declared = [
        (
            "alerts.email_to (WC_EMAIL_TO)",
            crate::config::alert_email_to(),
        ),
        (
            "alerts.skrzynka_mailbox (WC_ALERT_SKRZYNKA_MAILBOX)",
            crate::config::alert_skrzynka_mailbox(),
        ),
        (
            "alerts.skrzynka_organization (WC_ALERT_SKRZYNKA_ORGANIZATION)",
            crate::config::alert_skrzynka_organization(),
        ),
    ];
    let missing: Vec<&str> = declared
        .iter()
        .filter(|(_, value)| value.trim().is_empty())
        .map(|(name, _)| *name)
        .collect();
    if !missing.is_empty() {
        channel_failed(
            "skrzynka-configuration",
            &format!(
                "the operator chose the skrzynka channel (operator_contact) but the config \
                 declares no {}; name the destination and a mailbox Skrzynka holds \
                 (`skrzynka --organization <org> mailbox list`)",
                missing.join(", no ")
            ),
        );
        return None;
    }
    Some(SkrzynkaChannel {
        organization: crate::config::alert_skrzynka_organization()
            .trim()
            .to_string(),
        mailbox: crate::config::alert_skrzynka_mailbox().trim().to_string(),
        to: crate::config::alert_email_to().trim().to_string(),
    })
}

/// Resolve the most (SMS) channel: destination from the alerts item, Twilio
/// material through the selected credential store. Any gap degrades to no
/// channel with a structured failure line, never a panic.
pub(super) async fn resolve_most(phone: Option<String>) -> Option<MostChannel> {
    let phone = phone.filter(|value| !value.is_empty())?;
    // Field by field: the listener refuses a read that names no field, so the
    // whole-item form resolved to no channel and the deployment believed it
    // had no way to page anyone.
    let mut values = std::collections::BTreeMap::new();
    for name in [
        "account_sid",
        "auth_token",
        "api_version",
        "messaging_service_sid",
        "from_number",
    ] {
        match crate::credential_store::read_string("twilio", name).await {
            Ok(Some(value)) if !value.is_empty() => {
                values.insert(name, value);
            }
            Ok(_) => {}
            Err(err) => {
                channel_failed("most-configuration", &err.to_string());
                return None;
            }
        }
    }
    let field = |name: &str| values.get(name).cloned();
    match (
        field("account_sid"),
        field("auth_token"),
        field("api_version"),
    ) {
        (Some(account_sid), Some(auth_token), Some(api_version)) => Some(MostChannel {
            phone,
            account_sid,
            auth_token,
            api_version,
            messaging_service_sid: field("messaging_service_sid"),
            from_number: field("from_number"),
            api_base: TWILIO_API_BASE.to_string(),
        }),
        _ => {
            // Name what the grant actually returned: "needs three fields" hid
            // which read came back empty and cost a day of guessing.
            let missing: Vec<&str> = ["account_sid", "auth_token", "api_version"]
                .into_iter()
                .filter(|name| !values.contains_key(name))
                .collect();
            channel_failed(
                "most-configuration",
                &format!(
                    "most-twilio resolved {} of 5 fields; missing {}",
                    values.len(),
                    missing.join(", ")
                ),
            );
            None
        }
    }
}

/// A Google OAuth token for the Pub/Sub channel, from the fleet's GCP
/// provider grant.
pub(super) async fn gcp_token() -> Result<String, String> {
    let auth = crate::skarbiec::gcp_provider()
        .await
        .map_err(|error| format!("the GCP provider grant cannot be read from Skarbiec: {error}"))?;
    let token = auth
        .token(&[CLOUD_PLATFORM_SCOPE])
        .await
        .map_err(|error| format!("a GCP token for {CLOUD_PLATFORM_SCOPE} cannot be issued: {error}"))?;
    Ok(token.as_str().to_string())
}
