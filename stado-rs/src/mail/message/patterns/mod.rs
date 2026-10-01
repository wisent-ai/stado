//! Statically compiled scanners for the analysis pass.
//!
//! Every pattern is built once and shared: `URL_RE`, `AMOUNT_RE` and
//! `DATE_RE` harvest the published fields of a `MailAnalysis`.

use std::sync::LazyLock;

use regex::Regex;

/// Every scanner is declared in `patterns.json` beside this file, with
/// the reason it is shaped the way it is: which currencies this fleet is
/// billed in, and why a date is only recognised for this century.
fn declared(name: &str) -> Regex {
    let document: serde_json::Value = serde_json::from_str(include_str!("patterns.json"))
        .expect("patterns.json beside this file is valid JSON");
    let source = document[name]
        .as_str()
        .unwrap_or_else(|| panic!("patterns.json declares no {name}"));
    Regex::new(source).unwrap_or_else(|error| panic!("declared {name} pattern: {error}"))
}

pub(super) static URL_RE: LazyLock<Regex> = LazyLock::new(|| declared("url"));
pub(super) static AMOUNT_RE: LazyLock<Regex> = LazyLock::new(|| declared("amount"));
pub(super) static DATE_RE: LazyLock<Regex> = LazyLock::new(|| declared("date"));
