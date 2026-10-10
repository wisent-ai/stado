//! Which registry-managed service a logical service runs as on one target.
//!
//! A product the release policy rolls out by `replace` names a logical
//! service of the directory, and the unit the release agent restarts is the
//! managed service that route names: a fixed route names it in
//! `managed_service`; a placement-backed route leaves it absent and the
//! profile declares the unit per host under
//! `placement_profiles[].hosts.<target>.units.<service>`, in which case the
//! unit carries the logical name. The rollout and every reader of what that
//! rollout did (`stado release logs`) answer this question here, once, so a
//! reader cannot look at a unit the rollout never restarted.

use serde_json::Value;

use super::directory;

/// Why no managed unit could be named, as the sentence the caller reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedUnitRefusal {
    /// The registry declares no service directory.
    DirectoryAbsent,
    /// The directory is declared but cannot be read, with the reader's sentence.
    DirectoryUnreadable(String),
    /// The directory declares no route of that logical name.
    ServiceAbsent,
    /// The route is active on another host than the one asked about.
    ActiveElsewhere { active_host: String },
    /// The route names neither a managed service nor a placement profile.
    Undeclared,
    /// The route's placement profile declares no unit of that name on the
    /// target.
    ProfileLacksUnit { profile: String },
}

impl std::fmt::Display for ManagedUnitRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectoryAbsent => f.write_str("the registry declares no service directory"),
            Self::DirectoryUnreadable(sentence) => {
                write!(f, "the service directory cannot be read: {sentence}")
            }
            Self::ServiceAbsent => f.write_str("the service directory declares no such service"),
            Self::ActiveElsewhere { active_host } => {
                write!(f, "the service is active on {active_host}, not on the target asked about")
            }
            Self::Undeclared => {
                f.write_str("the service names neither a managed service nor a placement profile")
            }
            Self::ProfileLacksUnit { profile } => write!(
                f,
                "the service is placed by profile {profile:?}, which declares no unit of its name on the target"
            ),
        }
    }
}

/// The managed service `logical_service` runs as on `target`, or why none
/// can be named.
pub fn managed_unit_on(
    document: &Value,
    logical_service: &str,
    target: &str,
) -> Result<String, ManagedUnitRefusal> {
    let directory = directory(document)
        .map_err(ManagedUnitRefusal::DirectoryUnreadable)?
        .ok_or(ManagedUnitRefusal::DirectoryAbsent)?;
    let route = directory
        .services
        .get(logical_service)
        .ok_or(ManagedUnitRefusal::ServiceAbsent)?;
    if route.active_host != target {
        return Err(ManagedUnitRefusal::ActiveElsewhere {
            active_host: route.active_host.clone(),
        });
    }
    if let Some(unit) = route.managed_service.clone() {
        return Ok(unit);
    }
    let profile = route
        .placement_profile
        .as_deref()
        .ok_or(ManagedUnitRefusal::Undeclared)?;
    let declared = document
        .get("placement_profiles")
        .and_then(Value::as_array)
        .and_then(|profiles| {
            profiles
                .iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(profile))
        })
        .and_then(|entry| entry.pointer(&format!("/hosts/{target}/units/{logical_service}")))
        .is_some();
    if !declared {
        return Err(ManagedUnitRefusal::ProfileLacksUnit {
            profile: profile.to_string(),
        });
    }
    Ok(logical_service.to_string())
}
