//! The read-only Gmail REST surface.
//!
//! `auth` resolves the bearer token and `response` turns one HTTP reply into
//! JSON; the paging loop below is the only place that talks to the API.

mod auth;
mod response;

use serde_json::Value;

use super::message::analyze_message;
use super::{MailAnalysis, MailError};

use auth::gmail_token;
use response::response_json;

const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1";

pub struct GmailClient {
    http: reqwest::Client,
    token: String,
}

impl GmailClient {
    pub async fn from_env() -> Result<Self, MailError> {
        let token = gmail_token().await?;
        Ok(Self {
            http: reqwest::Client::new(),
            token,
        })
    }

    pub async fn analyze(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<MailAnalysis>, MailError> {
        let max_results = max_results.max(usize::from(true));
        let mut refs = Vec::new();
        let mut page_token: Option<String> = None;

        while refs.len() < max_results {
            let page_size = (max_results - refs.len())
                .min(usize::from(u8::MAX))
                .to_string();
            let mut request = self
                .http
                .get(format!("{GMAIL_BASE}/users/me/messages"))
                .bearer_auth(&self.token)
                .query(&[("q", query), ("maxResults", page_size.as_str())]);
            if let Some(token) = page_token.as_deref() {
                request = request.query(&[("pageToken", token)]);
            }
            let page = response_json(request.send().await?).await?;
            refs.extend(
                page.get("messages")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .map(str::to_string),
            );
            page_token = page
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if page_token.is_none() {
                break;
            }
        }

        refs.truncate(max_results);
        let mut messages = Vec::with_capacity(refs.len());
        for id in refs {
            let response = self
                .http
                .get(format!("{GMAIL_BASE}/users/me/messages/{id}"))
                .bearer_auth(&self.token)
                .query(&[("format", "full")])
                .send()
                .await?;
            messages.push(analyze_message(&response_json(response).await?));
        }
        Ok(messages)
    }
}
