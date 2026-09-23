//! Read-only Gmail search and deterministic billing/operations analysis.
//!
//! Authentication is resolved only from the `stado-gmail` Skarbiec item:
//! either a short-lived access token or centrally stored OAuth refresh
//! credentials. Stado never shells out to a cloud CLI and never modifies,
//! labels, archives, or sends messages.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use base64::Engine;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1";

static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").expect("static HTML regex"));
static SPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("static whitespace regex"));
static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s<>\"')\]]+"#).expect("static URL regex"));
static AMOUNT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:USD|EUR|GBP|PLN)\s*\d[\d,]*(?:\.\d{1,2})?|[$€£]\s*\d[\d,]*(?:\.\d{1,2})?|\d[\d,]*(?:\.\d{1,2})?\s*(?:USD|EUR|GBP|PLN)",
    )
    .expect("static amount regex")
});
static DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:20\d{2}-\d{2}-\d{2}|(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:tember)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)\s+\d{1,2}(?:st|nd|rd|th)?(?:,\s*20\d{2})?)\b",
    )
    .expect("static date regex")
});

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("Gmail authentication unavailable: {0}")]
    Auth(String),
    #[error("Gmail API HTTP {status}: {detail}")]
    Api { status: u16, detail: String },
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct MailAnalysis {
    pub id: String,
    pub thread_id: String,
    pub gmail_url: String,
    pub date: String,
    pub internal_date: Option<String>,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub labels: Vec<String>,
    pub snippet: String,
    pub categories: Vec<String>,
    pub amounts: Vec<String>,
    pub date_mentions: Vec<String>,
    pub links: Vec<String>,
    pub action_required: bool,
    pub action_signals: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MailAnalysisReport {
    pub query: String,
    pub message_count: usize,
    pub action_required_count: usize,
    pub categories: BTreeMap<String, usize>,
    pub amounts: Vec<String>,
    pub messages: Vec<MailAnalysis>,
}

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

pub fn summarize(query: &str, messages: Vec<MailAnalysis>) -> MailAnalysisReport {
    let mut categories = BTreeMap::new();
    let mut amounts = Vec::new();
    let mut seen_amounts = HashSet::new();
    for message in &messages {
        for category in &message.categories {
            categories
                .entry(category.clone())
                .and_modify(|count| *count += usize::from(true))
                .or_insert(usize::from(true));
        }
        for amount in &message.amounts {
            if seen_amounts.insert(amount.clone()) {
                amounts.push(amount.clone());
            }
        }
    }
    MailAnalysisReport {
        query: query.to_string(),
        message_count: messages.len(),
        action_required_count: messages
            .iter()
            .filter(|message| message.action_required)
            .count(),
        categories,
        amounts,
        messages,
    }
}

async fn gmail_token() -> Result<String, MailError> {
    let item = crate::skarbiec::Client::configured_item("stado-gmail")
        .await
        .map_err(|err| MailError::Auth(err.to_string()))?;
    if let Some(token) = item
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(token.trim().to_string());
    }
    let field = |name: &str| {
        item.get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                MailError::Auth(format!(
                    "Skarbiec item stado-gmail needs access_token or field {name}"
                ))
            })
    };
    let response = reqwest::Client::new()
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", field("client_id")?),
            ("client_secret", field("client_secret")?),
            ("refresh_token", field("refresh_token")?),
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

async fn response_json(response: reqwest::Response) -> Result<Value, MailError> {
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

fn analyze_message(message: &Value) -> MailAnalysis {
    let payload = message.get("payload").unwrap_or(&Value::Null);
    let plain = extract_body(payload, "text/plain");
    let html = extract_body(payload, "text/html");
    let body = if plain.trim().is_empty() {
        html_to_text(&html)
    } else {
        plain
    };
    let subject = header(payload, "subject");
    let from = header(payload, "from");
    let to = header(payload, "to");
    let date = header(payload, "date");
    let snippet = message
        .get("snippet")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let combined = format!("{subject}\n{from}\n{snippet}\n{body}");
    let lowered = combined.to_ascii_lowercase();

    let category_rules: &[(&str, &[&str])] = &[
        ("azure", &["azure", "microsoft cloud"]),
        (
            "startup_program",
            &[
                "microsoft for startups",
                "founders hub",
                "startup sponsorship",
            ],
        ),
        ("credits", &["credit", "sponsorship", "grant"]),
        (
            "billing",
            &["billing", "balance", "cost management", "payment"],
        ),
        ("invoice", &["invoice", "receipt"]),
        ("quota", &["quota", "capacity request", "service limit"]),
        (
            "security",
            &["security alert", "password", "verification code", "sign-in"],
        ),
    ];
    let categories = category_rules
        .iter()
        .filter(|(_, needles)| needles.iter().any(|needle| lowered.contains(*needle)))
        .map(|(category, _)| (*category).to_string())
        .collect();

    let action_rules = [
        "action required",
        "activate your",
        "redeem",
        "sign the",
        "sign in to",
        "verify your",
        "respond by",
        "expires",
        "deadline",
    ];
    let action_signals = action_rules
        .into_iter()
        .filter(|signal| lowered.contains(signal))
        .map(str::to_string)
        .collect::<Vec<_>>();

    let id = message
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    MailAnalysis {
        gmail_url: format!("https://mail.google.com/mail/u/me/#all/{id}"),
        id,
        thread_id: message
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        date,
        internal_date: message
            .get("internalDate")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|value| value.to_rfc3339()),
        from,
        to,
        subject,
        labels: message
            .get("labelIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        snippet,
        categories,
        amounts: regex_values(&AMOUNT_RE, &combined),
        date_mentions: regex_values(&DATE_RE, &combined),
        links: regex_values(&URL_RE, &combined),
        action_required: !action_signals.is_empty(),
        action_signals,
    }
}

fn header(payload: &Value, name: &str) -> String {
    payload
        .get("headers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|header| {
            header
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case(name))
        })
        .and_then(|header| header.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn extract_body(payload: &Value, wanted_mime: &str) -> String {
    let mut chunks = Vec::new();
    collect_body_parts(payload, wanted_mime, &mut chunks);
    chunks.join("\n")
}

fn collect_body_parts(part: &Value, wanted_mime: &str, chunks: &mut Vec<String>) {
    let mime = part
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if mime.eq_ignore_ascii_case(wanted_mime) {
        if let Some(data) = part.pointer("/body/data").and_then(Value::as_str) {
            if let Some(text) = decode_gmail_body(data) {
                chunks.push(text);
            }
        }
    }
    for child in part
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collect_body_parts(child, wanted_mime, chunks);
    }
}

fn decode_gmail_body(data: &str) -> Option<String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(data)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(data))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn html_to_text(html: &str) -> String {
    SPACE_RE
        .replace_all(
            &TAG_RE.replace_all(
                &html
                    .replace("&nbsp;", " ")
                    .replace("&amp;", "&")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">"),
                " ",
            ),
            " ",
        )
        .into_owned()
}

fn regex_values(regex: &Regex, text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut seen = HashSet::new();
    for found in regex.find_iter(text) {
        let value = found
            .as_str()
            .trim_end_matches(&['.', ',', ';', ':'][..])
            .to_string();
        if seen.insert(value.clone()) {
            values.push(value);
        }
    }
    values
}
