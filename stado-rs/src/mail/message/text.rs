//! Text flattening and deduplicated pattern harvesting.

use std::collections::HashSet;

use regex::Regex;

use super::patterns::{SPACE_RE, TAG_RE};

pub(super) fn html_to_text(html: &str) -> String {
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

pub(super) fn regex_values(regex: &Regex, text: &str) -> Vec<String> {
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
