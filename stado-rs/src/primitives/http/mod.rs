//! The body of an HTTP answer Stado reports as a refusal.
//!
//! A refusal's body is the remote side's own reason. Reading it as an empty
//! string whenever the read failed turned a body that broke off into an
//! empty reason, so the report said the remote answered nothing. Every
//! refusal reads its body here instead: the text, or the cause it could not
//! be read.

/// The refused answer's body, or the sentence naming why it could not be
/// read. The URL is stripped from the cause because the caller's report
/// already names what was asked.
pub async fn refused_body(response: reqwest::Response) -> String {
    match response.text().await {
        Ok(text) => text,
        Err(error) => format!("(the body could not be read: {})", error.without_url()),
    }
}
