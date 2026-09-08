//! The two read shapes every diagnosis needs: one ARM GET, and one ARM GET
//! followed to the end of its `nextLink` chain.

use serde_json::{json, Value};

use super::super::CmdError;

pub(super) async fn azure_get_json(
    http: &reqwest::Client,
    access_token: &str,
    url: &str,
) -> Result<Value, CmdError> {
    let response = http.get(url).bearer_auth(access_token).send().await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let body: Value =
        serde_json::from_str(&text).unwrap_or_else(|_| json!({"detail": text.trim()}));
    if status.is_success() {
        Ok(body)
    } else {
        Err(CmdError::click(format!(
            "Azure request failed with HTTP {status}: {}",
            body.pointer("/error/message")
                .or_else(|| body.get("detail"))
                .and_then(Value::as_str)
                .unwrap_or("unknown ARM error")
        )))
    }
}

pub(super) async fn azure_collection(
    http: &reqwest::Client,
    access_token: &str,
    first_url: String,
) -> Result<Vec<Value>, CmdError> {
    let mut url = Some(first_url);
    let mut rows = Vec::new();
    while let Some(page_url) = url.take() {
        let page = azure_get_json(http, access_token, &page_url).await?;
        rows.extend(
            page.get("value")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        url = page
            .get("nextLink")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    Ok(rows)
}
