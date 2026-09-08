//! The compiled matchers and the Microsoft sender-domain table the reply
//! gate consults: region extraction from a ticket title, billing-decline
//! detection, HTML/whitespace snippet cleanup, and the
//! communication-name sanitizer.

use std::sync::LazyLock;

/// Python `_REGION_RE`.
pub(super) fn region_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\(([^)]+)\)\s*$").expect("static regex compiles"));
    &RE
}

/// Python `_BILLING_DECLINE_RE`.
///
/// Patterns Azure Capacity CX uses when the issue is BILLING (payment
/// history, bank decline, outstanding balance), not a request for
/// customer info. Auto-replying the standard 5-answer template against a
/// billing-decline message is useless — the operator has to fix the
/// payment side before any quota can be granted. Detect and route those
/// to a skip_billing_decline action instead of replying.
pub(super) fn billing_decline_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::RegexBuilder::new(
            r"insufficient payment history|bank decline|outstanding balance|unpaid invoice|payment issues|pay now to resolve|billing issue",
        )
        .case_insensitive(true)
        .build()
        .expect("static regex compiles")
    });
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
pub(super) const MS_SENDER: [&str; 2] = ["techsupport.microsoft.com", "microsoft.com"];
