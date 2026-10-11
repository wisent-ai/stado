//! The two channel resolvers with a story of their own — Resend's
//! config-or-vault destination and most's provider-grant Twilio material —
//! and the Pub/Sub OAuth fetch.

use super::super::{
    channel_failed, MostChannel, ResendChannel, CLOUD_PLATFORM_SCOPE, DEFAULT_EMAIL_FROM,
    RESEND_URL, TWILIO_API_BASE,
};

/// Resolve the Resend email channel: destination and sender from the config
/// document (`alerts.email_to`, `alerts.email_from`, or their `WC_EMAIL_*`
/// env names), falling back to the alerts item for a deployment that keeps
/// them in the vault; key from the vault item named by `alerts.resend_item`.
/// A gap degrades to no channel with a structured failure line.
pub(super) async fn resolve_resend(
    to: Option<String>,
    from: Option<String>,
) -> Option<ResendChannel> {
    let configured = |value: &str| (!value.is_empty()).then(|| value.to_string());
    let Some(to) = configured(crate::config::alert_email_to())
        .or(to)
        .filter(|value| !value.is_empty())
    else {
        channel_failed(
            "resend-configuration",
            "no destination: set alerts.email_to (or WC_EMAIL_TO)",
        );
        return None;
    };
    // The coordinator's grant does not carry the resend key, and reading with
    // it turned the only configured channel into no channel at all.
    let vault = crate::skarbiec::Client::stado()
        .map_err(|err| channel_failed("resend-configuration", &err.to_string()))
        .ok()?;
    let item = crate::config::alert_resend_item();
    let field = crate::config::alert_resend_field();
    let key = match vault.read_string(item, field).await {
        Ok(Some(value)) if !value.is_empty() => value,
        // `alerts.resend_item` is read as the role the key's item plays. An
        // answer without a value used to read "is empty" while the key sat in
        // the vault untagged, so the operator was paged by nothing and told
        // to look at a field that was full.
        Ok(_) => {
            channel_failed(
                "resend-configuration",
                &format!(
                    "the vault gave no {field} for role {item}: no live item carries \
                     stado:role:{item}, or that item's {field} is empty; tag the Resend key's \
                     item (`stado credentials item retag --host <vault owner> <item> --tags \
                     stado:role:{item}`) and grant the read (`stado credentials grant add \
                     --host <vault owner> --role {item} --field {field} --token-file <stado token \
                     file> stado`)"
                ),
            );
            return None;
        }
        Err(err) => {
            channel_failed("resend-configuration", &err.to_string());
            return None;
        }
    };
    Some(ResendChannel {
        api_key: key,
        to,
        from: configured(crate::config::alert_email_from())
            .or(from)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_EMAIL_FROM.to_string()),
        url: RESEND_URL.to_string(),
    })
}

/// Resolve the most (SMS) channel: destination from the alerts item, Twilio
/// material through the `most` integration provider grant. Any gap degrades
/// to no channel with a structured failure line, never a panic.
pub(super) async fn resolve_most(phone: Option<String>) -> Option<MostChannel> {
    let phone = phone.filter(|value| !value.is_empty())?;
    let provider = crate::skarbiec::Client::stado()
        .map_err(|err| channel_failed("most-configuration", &err.to_string()))
        .ok()?;
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
        match provider.read_string("twilio", name).await {
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
        .map_err(|e| e.to_string())?;
    let token = auth
        .token(&[CLOUD_PLATFORM_SCOPE])
        .await
        .map_err(|e| e.to_string())?;
    Ok(token.as_str().to_string())
}
