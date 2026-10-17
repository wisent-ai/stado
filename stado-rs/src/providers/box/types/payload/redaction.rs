//! Redaction of Boat payload text: URL tokens and Authorization values.
//!
//! `safe_text` and `safe_text_limited`, which every text field of the
//! `errors` component passes through at construction. The account's own API
//! key is removed by the transport, which holds it, before any answer
//! becomes text here.

use std::sync::LazyLock;

use regex::Regex;

fn url_token_pattern() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)([?&](?:_token|token|key|access_token)=)[^&\s]+")
            .expect("static regex compiles")
    });
    &RE
}

fn authorization_pattern() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)authorization\s*[:=]\s*[^,;\s]+").expect("static regex compiles")
    });
    &RE
}

/// Single-line text with URL tokens and Authorization values redacted. An
/// empty `value` is reported as `default_text`, the sentence the caller
/// states for an answer that named nothing.
pub fn safe_text(value: &str, default_text: &str) -> String {
    let text = if value.is_empty() {
        default_text
    } else {
        value
    };
    let text = text.replace(['\r', '\n'], " ");
    let text = url_token_pattern().replace_all(&text, "$1[REDACTED]");
    authorization_pattern()
        .replace_all(&text, "Authorization=[REDACTED]")
        .into_owned()
}

/// [`safe_text`] cut to the first `limit` characters the caller allows.
pub fn safe_text_limited(value: &str, default_text: &str, limit: usize) -> String {
    safe_text(value, default_text).chars().take(limit).collect()
}
