//! Statically compiled scanners for the analysis pass.
//!
//! Every pattern is built once and shared: `TAG_RE` and `SPACE_RE` drive
//! HTML flattening, and `URL_RE`, `AMOUNT_RE` and `DATE_RE` harvest the
//! published fields of a `MailAnalysis`.

use std::sync::LazyLock;

use regex::Regex;

pub(super) static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").expect("static HTML regex"));
pub(super) static SPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("static whitespace regex"));
pub(super) static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s<>\"')\]]+"#).expect("static URL regex"));
pub(super) static AMOUNT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:USD|EUR|GBP|PLN)\s*\d[\d,]*(?:\.\d{1,2})?|[$€£]\s*\d[\d,]*(?:\.\d{1,2})?|\d[\d,]*(?:\.\d{1,2})?\s*(?:USD|EUR|GBP|PLN)",
    )
    .expect("static amount regex")
});
pub(super) static DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:20\d{2}-\d{2}-\d{2}|(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:tember)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)\s+\d{1,2}(?:st|nd|rd|th)?(?:,\s*20\d{2})?)\b",
    )
    .expect("static date regex")
});
