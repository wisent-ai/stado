//! Argument validators run before a value reaches a Cloudflare API path, a DNS
//! record name or a tunnel origin URL.

use crate::cli::CmdError;

pub(in crate::cli::cloudflare) fn validate_zone_hostname(
    zone: &str,
    hostname: &str,
) -> Result<(), CmdError> {
    validate_dns_name("zone", zone)?;
    validate_dns_name("hostname", hostname)?;
    if !belongs_to_zone(hostname, zone) {
        return Err(CmdError::usage(format!(
            "hostname {hostname:?} is outside zone {zone:?}"
        )));
    }
    Ok(())
}

pub(in crate::cli::cloudflare) fn belongs_to_zone(hostname: &str, zone: &str) -> bool {
    hostname == zone
        || hostname
            .strip_suffix(zone)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

pub(in crate::cli::cloudflare) fn validate_api_component(
    label: &str,
    value: &str,
) -> Result<(), CmdError> {
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "Cloudflare {label} contains characters that cannot form an API path"
    )))
}

pub(in crate::cli::cloudflare) fn validate_dns_name(
    label: &str,
    value: &str,
) -> Result<(), CmdError> {
    let valid = !value.is_empty()
        && value.len() <= 253
        && value == value.to_ascii_lowercase()
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
                })
                && !part.starts_with('-')
                && !part.ends_with('-')
        });
    if valid {
        return Ok(());
    }
    Err(CmdError::usage(format!(
        "Cloudflare {label} must be a lowercase DNS name"
    )))
}

pub(in crate::cli::cloudflare) fn validate_origin(origin: &str) -> Result<(), CmdError> {
    let parsed = url::Url::parse(origin)
        .map_err(|error| CmdError::usage(format!("Cloudflare origin URL is invalid: {error}")))?;
    if matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none()
    {
        return Ok(());
    }
    Err(CmdError::usage(
        "Cloudflare origin must be an HTTP(S) URL without credentials or a fragment",
    ))
}
