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
/// (`[user@]host[:port]`, bracketed IPv6 supported). A bracket that never
/// closes leaves the destination as written, which then matches no host.
pub fn ssh_hostname(value: &str) -> String {
    let destination = value.trim();
    let host_and_port = match destination.rsplit_once('@') {
        Some((_, host)) => host,
        None => destination,
    };
    let host = match host_and_port.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((inner, _)) => inner,
            None => host_and_port,
        },
        None => match host_and_port.split_once(':') {
            Some((host, _)) => host,
            None => host_and_port,
        },
    };
    normalize_hostname(host)
}

// ---------------------------------------------------------------------------
// validation.py — registry-v2 contract
// ---------------------------------------------------------------------------

/// Schema version required of registry documents: the one the bundled empty
/// registry (`data/fleet/registry.json`) declares, so the version is written
/// in one place and the document Stado ships always satisfies it.
pub fn registry_schema_version() -> i64 {
    static VERSION: std::sync::LazyLock<i64> = std::sync::LazyLock::new(|| {
        let bundled: Value =
            serde_json::from_str(include_str!("../../../data/fleet/registry.json"))
                .expect("the bundled registry is the JSON this build was compiled with");
        bundled["schema_version"]
            .as_i64()
            .expect("the bundled registry declares its schema_version")
    });
    *VERSION
}

/// Raised when a registry does not satisfy the version contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RegistryValidationError(pub String);

pub(crate) fn verr(location: &str, message: &str) -> RegistryValidationError {
    RegistryValidationError(format!("{location}: {message}"))
}

/// Python `repr()` of a string list, for the registry refusals that quote
/// the words a field accepts: the one copy in `models::python_compat`.
pub(crate) use crate::models::py_list_repr;

/// `^[a-z0-9](?:[a-z0-9._-]*[a-z0-9])?$` hand-rolled (no regex dependency).
pub(crate) fn is_target_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    let alnum = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    bytes.first().is_some_and(alnum)
        && bytes.last().is_some_and(alnum)
        && bytes
            .iter()
            .all(|b| alnum(b) || matches!(b, b'.' | b'_' | b'-'))
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
