//! The shapes that decide what counts as a credential: which names hold one,
//! which values look like key material, and how a `NAME=VALUE` or
//! `"NAME": "VALUE"` pair is pulled out of a line of payload text.

/// Whether a name looks like it holds a credential rather than a setting.
pub(in crate::transcripts) fn name_suggests_secret(name: &str) -> bool {
    const MARKERS: &[&str] = &[
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "PRIVATE",
        "SERVICE_ROLE",
        "ACCESS",
        "BEARER",
        "SIGNING",
        "UNLOCK",
        "DSN",
        "WEBHOOK",
    ];
    let upper = name.to_ascii_uppercase();
    MARKERS.iter().any(|marker| upper.contains(marker))
}

/// Whether a value looks like key material. Deliberately structural — no
/// provider prefix list to fall behind, and no attempt to judge what the value
/// unlocks.
pub(in crate::transcripts) fn value_looks_secret(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    // Placeholders and references are the common false positive: `op://…`,
    // `${VAR}`, `<redacted>`, an empty template, a masked lake field.
    let placeholder = value.contains("://")
        || value.contains("${")
        || value.contains("masked:")
        || value.starts_with('<')
        || value.contains("REDACTED")
        || value.contains("EXAMPLE")
        || value.contains("your-");
    if placeholder {
        return false;
    }
    !value.contains(char::is_whitespace)
}

/// Pull `NAME=VALUE` and `"NAME": "VALUE"` pairs out of one line of payload
/// text. Environment dumps use the first shape, JSON-rendered results the
/// second.
pub(in crate::transcripts) fn pairs_in_line(line: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (index, _) in line.match_indices('=') {
        let (left, right) = line.split_at(index);
        let name: String = left
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        let raw = right.trim_start_matches('=');
        let value: String = raw
            .trim_start_matches('"')
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '"' && *c != ',')
            .collect();
        if !name.is_empty() && !value.is_empty() {
            found.push((name, value));
        }
    }
    for (index, _) in line.match_indices("\": \"") {
        let (left, right) = line.split_at(index);
        let name: String = left
            .chars()
            .rev()
            .take_while(|c| *c != '"')
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        let value: String = right
            .trim_start_matches("\": \"")
            .chars()
            .take_while(|c| *c != '"')
            .collect();
        if !name.is_empty() && !value.is_empty() {
            found.push((name, value));
        }
    }
    found
}
