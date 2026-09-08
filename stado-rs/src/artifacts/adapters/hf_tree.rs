//! Listing one immutable Hugging Face dataset revision over the HF HTTP
//! API, and the injectable fetcher the adapter takes it through.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Hugging Face tree listing
// ---------------------------------------------------------------------------

/// Failure of the HF tree listing. Python reports
/// `f"{type(exc).__name__}: {exc}"`; `kind` carries the exception-style
/// label ("HTTPError" for non-2xx, "RequestError" for transport failures,
/// "RuntimeError" for a non-list body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFetchError {
    pub kind: &'static str,
    pub message: String,
}

impl std::fmt::Display for TreeFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

/// Python `urllib.parse.quote(value, safe=...)`: UTF-8 percent-encoding,
/// unreserved `A-Za-z0-9_.-~` never escaped, uppercase hex.
fn quote(value: &str, safe_slash: bool) -> String {
    let mut out = String::new();
    for &byte in value.as_bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'.' | b'-' | b'~')
            || (safe_slash && byte == b'/');
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Python `_next_link`: pull `rel="next"` out of an RFC 5988 Link header.
fn next_link(header: &str) -> String {
    for part in header.split(',') {
        let bits: Vec<&str> = part.trim().split(';').collect();
        if bits.len() > 1 && bits[1..].iter().any(|bit| bit.trim() == "rel=\"next\"") {
            return bits[0]
                .trim()
                .trim_matches(|c| c == '<' || c == '>')
                .to_string();
        }
    }
    String::new()
}

/// List every file at one immutable Hugging Face dataset revision. Follows
/// `rel="next"` pagination and uses `stado-huggingface/token` from Skarbiec
/// when present.
pub async fn fetch_hf_tree(repo: &str, revision: &str) -> Result<Vec<String>, TreeFetchError> {
    let encoded_repo = quote(repo, true);
    let encoded_revision = quote(revision, false);
    let mut url = format!(
        "https://huggingface.co/api/datasets/{encoded_repo}/tree/{encoded_revision}\
         ?recursive=true&expand=false&limit=1000"
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("stado-artifacts/1")
        .build()
        .map_err(|exc| TreeFetchError {
            kind: "RequestError",
            message: exc.to_string(),
        })?;
    let token = crate::skarbiec::read_string("stado-huggingface", "token")
        .await
        .map_err(|exc| TreeFetchError {
            kind: "AuthenticationError",
            message: exc.to_string(),
        })?
        .unwrap_or_default();

    let mut paths: Vec<String> = Vec::new();
    while !url.is_empty() {
        let mut request = client.get(&url);
        if !token.is_empty() {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|exc| TreeFetchError {
                kind: if exc.is_status() {
                    "HTTPError"
                } else {
                    "RequestError"
                },
                message: exc.to_string(),
            })?;
        let link = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let page: Value = response.json().await.map_err(|exc| TreeFetchError {
            kind: "RequestError",
            message: exc.to_string(),
        })?;
        let Some(items) = page.as_array() else {
            return Err(TreeFetchError {
                kind: "RuntimeError",
                message: "Hugging Face tree response is not a list".to_string(),
            });
        };
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("file") {
                if let Some(path) = item.get("path").and_then(Value::as_str) {
                    if !path.is_empty() {
                        paths.push(path.to_string());
                    }
                }
            }
        }
        url = next_link(&link);
    }
    Ok(paths)
}

/// Injectable tree fetcher (Python passes `tree_fetcher` to the adapter
/// constructor for tests).
pub type TreeFetcher = Arc<
    dyn Fn(String, String) -> BoxFuture<'static, Result<Vec<String>, TreeFetchError>> + Send + Sync,
>;
