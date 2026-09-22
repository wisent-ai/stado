//! Statically compiled scanners for the analysis pass.
//!
//! Every pattern is built once and shared: `TAG_RE` and `SPACE_RE` drive
//! HTML flattening, and `URL_RE`, `AMOUNT_RE` and `DATE_RE` harvest the
//! published fields of a `MailAnalysis`.

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

pub(super) static TAG_RE: LazyLock<Regex> = LazyLock::new(|| declared("tag"));
pub(super) static SPACE_RE: LazyLock<Regex> = LazyLock::new(|| declared("space"));
pub(super) static URL_RE: LazyLock<Regex> = LazyLock::new(|| declared("url"));
pub(super) static AMOUNT_RE: LazyLock<Regex> = LazyLock::new(|| declared("amount"));
pub(super) static DATE_RE: LazyLock<Regex> = LazyLock::new(|| declared("date"));

#[cfg(test)]
mod tests {
    use super::{AMOUNT_RE, DATE_RE, SPACE_RE, TAG_RE, URL_RE};

    /// A line out of a real hosting invoice mail.
    const BODY: &str = "<p>Your <b>invoice</b> for  March 3, 2026 is USD 1,204.55 \
         (EUR 1,110.00). Pay at https://billing.example.com/inv/9f2?x=1 or reply.</p>";

    #[test]
    fn the_declared_scanners_load_and_read_a_message() {
        let without_tags = TAG_RE.replace_all(BODY, " ").into_owned();
        let flat = SPACE_RE.replace_all(&without_tags, " ").into_owned();
        assert!(!flat.contains('<'), "{flat}");
        assert!(!flat.contains("  "), "{flat}");
        let amounts: Vec<&str> = AMOUNT_RE.find_iter(&flat).map(|m| m.as_str()).collect();
        assert_eq!(amounts, vec!["USD 1,204.55", "EUR 1,110.00"], "{flat}");
        let dates: Vec<&str> = DATE_RE.find_iter(&flat).map(|m| m.as_str()).collect();
        assert_eq!(dates, vec!["March 3, 2026"], "{flat}");
        let links: Vec<&str> = URL_RE.find_iter(&flat).map(|m| m.as_str()).collect();
        assert_eq!(
            links,
            vec!["https://billing.example.com/inv/9f2?x=1"],
            "{flat}"
        );
    }

    #[test]
    fn an_iso_date_is_read_and_a_release_number_is_not() {
        let dates: Vec<&str> = DATE_RE
            .find_iter("scheduled 2026-03-03, build 1999-alpha, version 3.14")
            .map(|m| m.as_str())
            .collect();
        assert_eq!(dates, vec!["2026-03-03"]);
    }

    #[test]
    fn a_currency_nobody_bills_us_in_is_not_an_amount() {
        assert!(AMOUNT_RE.find("total JPY 4500").is_none());
        assert_eq!(
            AMOUNT_RE.find("total 89.99 PLN").map(|m| m.as_str()),
            Some("89.99 PLN"),
        );
    }
}
