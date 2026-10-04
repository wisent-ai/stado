use crate::targets::*;

// ---------------------------------------------------------------------------
// validation.py — hostname normalization
// ---------------------------------------------------------------------------

/// Return the canonical form used for host identity comparisons.
pub fn normalize_hostname(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .trim_end_matches('.')
        .to_string()
}

/// Extract and normalize a hostname from a legacy SSH destination
/// (`[user@]host[:port]`, bracketed IPv6 supported).
pub fn ssh_hostname(value: &str) -> String {
    let host_and_port = value.trim().rsplit('@').next().unwrap_or("");
    let host = if host_and_port.starts_with('[') {
        match host_and_port.find(']') {
            Some(closing) if closing > 1 => &host_and_port[1..closing],
            _ => "",
        }
    } else {
        host_and_port.split(':').next().unwrap_or("")
    };
    normalize_hostname(host)
}

// ---------------------------------------------------------------------------
// validation.py — registry-v2 contract
// ---------------------------------------------------------------------------

/// Schema version required of registry documents (Python
/// `_REGISTRY_VERSION`).
pub const REGISTRY_SCHEMA_VERSION: i64 = 2;

/// Raised when a registry does not satisfy the version 2 contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RegistryValidationError(pub String);

pub(crate) fn verr(location: &str, message: &str) -> RegistryValidationError {
    RegistryValidationError(format!("{location}: {message}"))
}

/// Python `repr()` of a sorted string list: `['a', 'b']`.
pub(crate) fn py_list_repr(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("'{i}'")).collect();
    format!("[{}]", quoted.join(", "))
}

/// `^[a-z0-9](?:[a-z0-9._-]*[a-z0-9])?$` hand-rolled (no regex dependency).
pub(crate) fn is_target_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !bytes.is_empty()
        && alnum(bytes[0])
        && alnum(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|&b| alnum(b) || matches!(b, b'.' | b'_' | b'-'))
}

/// `^[a-z0-9_]+$` hand-rolled.
fn is_action(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

pub(crate) fn validate_action_list(
    value: &Value,
    location: &str,
) -> Result<(), RegistryValidationError> {
    let items = value
        .as_array()
        .ok_or_else(|| verr(location, "must be an array"))?;
    let mut seen: HashSet<&str> = HashSet::new();
    for (index, action) in items.iter().enumerate() {
        let item_location = format!("{location}[{index}]");
        let action = match action.as_str() {
            Some(a) if !a.is_empty() && a == a.trim() => a,
            _ => {
                return Err(verr(
                    &item_location,
                    "must be a non-empty string without surrounding whitespace",
                ))
            }
        };
        if !is_action(action) {
            return Err(verr(
                &item_location,
                "must be an exact lowercase action identifier; wildcard grants are forbidden",
            ));
        }
        if !seen.insert(action) {
            return Err(verr(
                &item_location,
                &format!("duplicate action '{action}'"),
            ));
        }
    }
    Ok(())
}
