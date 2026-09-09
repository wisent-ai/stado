//! Redaction of Box payload text: box keys, URL tokens, Authorization.
//!
//! Python `_BOX_KEY_PATTERN`, `_URL_TOKEN_PATTERN`,
//! `_AUTHORIZATION_PATTERN` and `safe_text`, which every text field of the
//! `errors` component passes through at construction.

use std::sync::LazyLock;

use regex::Regex;

fn box_key_pattern() -> &'static Regex {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"box_[A-Za-z0-9_-]+").expect("static regex compiles"));
    &RE
}

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

/// Python `safe_text`: single-line text with box keys, URL tokens and
/// Authorization headers redacted, truncated to `limit` characters.
/// An empty `value` falls back to `default_text` (Python `value or default`).
pub fn safe_text(value: &str, default_text: &str) -> String {
    safe_text_limited(value, default_text, 512)
}

/// [`safe_text`] with an explicit limit (Python `limit=512` default).
pub fn safe_text_limited(value: &str, default_text: &str, limit: usize) -> String {
    let text = if value.is_empty() {
        default_text
    } else {
        value
    };
    let text = text.replace(['\r', '\n'], " ");
    let text = box_key_pattern().replace_all(&text, "[REDACTED]");
    let text = url_token_pattern().replace_all(&text, "$1[REDACTED]");
    let text = authorization_pattern().replace_all(&text, "Authorization=[REDACTED]");
    // Python text[:limit] slices code points, not bytes.
    text.chars().take(limit).collect()
}
