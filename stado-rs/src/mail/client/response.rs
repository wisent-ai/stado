//! Status checking and JSON decoding for one Gmail API response.

use serde_json::Value;

use crate::mail::MailError;

pub(super) async fn response_json(response: reqwest::Response) -> Result<Value, MailError> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(MailError::Api {
            status: status.as_u16(),
            detail: text,
        });
    }
    serde_json::from_str(&text).map_err(|err| MailError::Api {
        status: status.as_u16(),
        detail: format!("response is not JSON: {err}"),
    })
}
