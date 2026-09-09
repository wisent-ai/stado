//! Gmail credential resolution from the `stado-gmail` Skarbiec item.
//!
//! A stored `access_token` wins; otherwise the OAuth refresh triple is read
//! and exchanged once per client.

use serde_json::Value;

use crate::mail::MailError;

async fn gmail_field(name: &'static str) -> Result<Option<String>, MailError> {
    crate::skarbiec::read_string("stado-gmail", name)
        .await
        .map_err(|error| MailError::Auth(error.to_string()))
        .map(|value| {
            value.and_then(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
        })
}

async fn required_gmail_field(name: &'static str) -> Result<String, MailError> {
    gmail_field(name).await?.ok_or_else(|| {
        MailError::Auth(format!(
            "Skarbiec item stado-gmail needs access_token or field {name}"
        ))
    })
}

pub(super) async fn gmail_token() -> Result<String, MailError> {
    if let Some(token) = gmail_field("access_token").await? {
        return Ok(token);
    }
    let (client_id, client_secret, refresh_token) = tokio::try_join!(
        required_gmail_field("client_id"),
        required_gmail_field("client_secret"),
        required_gmail_field("refresh_token"),
    )?;
    let response = reqwest::Client::new()
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token.as_str()),
        ])
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(MailError::Auth(format!(
            "Google OAuth refresh failed with HTTP {status}: {body}"
        )));
    }
    body.get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| MailError::Auth("Google OAuth refresh response has no access_token".into()))
}
