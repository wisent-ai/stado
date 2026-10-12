//! The compiled matchers and the Microsoft sender-domain table the reply
//! gate consults: region extraction from a ticket title, HTML/whitespace
//! snippet cleanup, and the communication-name sanitizer. Whether
//! Microsoft's message holds the request on billing is read by a model
//! (`respond`), not matched here.

use std::sync::LazyLock;

/// Python `_REGION_RE`.
pub(super) fn region_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\(([^)]+)\)\s*$").expect("static regex compiles"));
    &RE
}

pub(super) fn html_tag_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<[^>]+>").expect("static regex compiles"));
    &RE
}

pub(super) fn ws_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s+").expect("static regex compiles"));
    &RE
}

pub(super) fn comm_name_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"[^A-Za-z0-9-]").expect("static regex compiles"));
    &RE
}

/// Python `_MS_SENDER`.
pub(super) const MS_SENDER: &[&str] = &["techsupport.microsoft.com", "microsoft.com"];
