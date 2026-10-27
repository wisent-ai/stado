//! Field-level validation shared by the registry placement contract.

use std::path::{Component, Path};

use super::model::{PlacementProbe, PlacementUnit};
use crate::deploy::service::UnitKind;

fn identifier(value: &str) -> bool {
    crate::primitives::word::lowercase_identifier(value)
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
    match managed.kind {
        UnitKind::Launchd if path.extension().and_then(|part| part.to_str()) != Some("plist") => {
            Err(format!("{location}.path: launchd units must end in .plist"))
        }
        UnitKind::Systemd if !managed.unit.ends_with(".service") => Err(format!(
            "{location}.unit: systemd unit names must end in .service"
        )),
        UnitKind::Launchd | UnitKind::Systemd => Ok(()),
    }
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
        return Err(format!(
            "{location}: must be a clean path relative to its root ($HOME, or the host's work_root with root: work)"
        ));
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
    // A host that is a name, not an address, is never loopback here: the
    // probe is dialled, and a name resolves wherever the resolver says. The
    // URL parser already says which kind of host it read.
    let loopback = match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) | None => false,
    };
    if !loopback || parsed.port().is_none() {
        return Err(format!(
            "{location}.url: must use a loopback address and explicit port"
        ));
    }
    Ok(())
}
