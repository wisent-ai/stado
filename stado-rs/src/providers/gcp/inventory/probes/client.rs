//! Probe execution: one bearer-authenticated request per probe, paginated to
//! exhaustion, with every failure mode turned into report data.

use std::collections::BTreeSet;

use reqwest::Method;
use serde_json::{json, Value};

use crate::providers::gcp::inventory::fields::encode;
use crate::providers::gcp::inventory::ProbeReport;

use super::outcome::{failed_api, failed_transport, successful};
use super::ProbeSpec;

#[derive(Clone)]
pub(in crate::providers::gcp::inventory) struct Client {
    pub(in crate::providers::gcp::inventory) http: reqwest::Client,
    pub(in crate::providers::gcp::inventory) token: String,
}

impl Client {
    pub(in crate::providers::gcp::inventory) async fn run(&self, spec: ProbeSpec) -> ProbeReport {
        let mut url = spec.url.clone();
        let mut merged = None;
        let mut seen_tokens = BTreeSet::new();
        loop {
            let mut request = self
                .http
                .request(spec.method.clone(), &url)
                .bearer_auth(&self.token)
                .header(reqwest::header::ACCEPT, "application/json");
            if let Some(body) = &spec.body {
                request = request.json(body);
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => return failed_transport(spec, error.to_string()),
            };
            let status = response.status();
            let body = match response.text().await {
                Ok(body) => body,
                Err(error) => return failed_transport(spec, error.to_string()),
            };
            if !status.is_success() {
                return failed_api(spec, status, &body);
            }
            let value = match serde_json::from_str::<Value>(&body) {
                Ok(value) => value,
                Err(error) => {
                    return failed_transport(spec, format!("invalid JSON response: {error}"))
                }
            };
            let next_token = if spec.method == Method::GET {
                value
                    .get("nextPageToken")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else {
                None
            };
            match &mut merged {
                Some(existing) => merge_page(existing, value),
                None => merged = Some(value),
            }
            let Some(token) = next_token else {
                return successful(spec, merged.unwrap_or_else(|| json!({})));
            };
            if !seen_tokens.insert(token.clone()) {
                return failed_transport(spec, format!("pagination token repeated: {token}"));
            }
            let separator = if spec.url.contains('?') { '&' } else { '?' };
            url = format!("{}{separator}pageToken={}", spec.url, encode(&token));
        }
    }
}

fn merge_page(target: &mut Value, page: Value) {
    match (target, page) {
        (Value::Array(target), Value::Array(mut page)) => target.append(&mut page),
        (Value::Object(target), Value::Object(page)) => {
            for (key, value) in page {
                if key == "nextPageToken" {
                    continue;
                }
                match target.get_mut(&key) {
                    Some(existing) => merge_page(existing, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        _ => {}
    }
}
