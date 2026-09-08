//! Field-level validation shared by the registry placement contract.

use std::path::{Component, Path};

use super::model::{PlacementProbe, PlacementUnit};

fn identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    let edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    !bytes.is_empty()
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| edge(*byte) || matches!(byte, b'.' | b'_' | b'-'))
}

pub(in crate::placement) fn validate_identifier(value: &str, location: &str) -> Result<(), String> {
    if identifier(value) {
        Ok(())
    } else {
        Err(format!(
            "{location}: must be a lowercase identifier without empty edges"
        ))
    }
}

pub(in crate::placement) fn validate_unit(
    unit: &PlacementUnit,
    location: &str,
) -> Result<(), String> {
    validate_identifier(&unit.name, &format!("{location}.name"))?;
    let Some(managed) = unit.managed() else {
        let owner = unit
            .release_controlled()
            .expect("placement lifecycle is exhaustive");
        return validate_identifier(&owner.product, &format!("{location}.product"));
    };
    if managed.unit.is_empty() || managed.unit.chars().any(char::is_control) {
        return Err(format!("{location}.unit: must be a non-empty unit name"));
    }
    if managed.kind != "launchd" && managed.kind != "systemd" {
        return Err(format!(
            "{location}.kind: must be one of ['launchd', 'systemd']"
        ));
    }
    let path = Path::new(&managed.path);
    if !path.is_absolute()
        || managed.path.chars().any(char::is_control)
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "{location}.path: must be an absolute path without '..'"
        ));
    }
    if managed.kind == "launchd" && path.extension().and_then(|part| part.to_str()) != Some("plist")
    {
        return Err(format!("{location}.path: launchd units must end in .plist"));
    }
    if managed.kind == "systemd" && !managed.unit.ends_with(".service") {
        return Err(format!(
            "{location}.unit: systemd unit names must end in .service"
        ));
    }
    Ok(())
}

pub(in crate::placement) fn validate_state_path(path: &str, location: &str) -> Result<(), String> {
    let parsed = Path::new(path);
    if path.is_empty()
        || parsed.is_absolute()
        || path.starts_with('~')
        || path.chars().any(char::is_control)
        || parsed.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("{location}: must be a clean, $HOME-relative path"));
    }
    Ok(())
}

pub(in crate::placement) fn validate_probe(
    probe: &PlacementProbe,
    location: &str,
) -> Result<(), String> {
    let parsed = url::Url::parse(&probe.url)
        .map_err(|error| format!("{location}.url: invalid URL: {error}"))?;
    if parsed.scheme() != "http" {
        return Err(format!("{location}.url: must use http on loopback"));
    }
    let loopback = parsed
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if !loopback || parsed.port().is_none() {
        return Err(format!(
            "{location}.url: must use a loopback address and explicit port"
        ));
    }
    Ok(())
}
