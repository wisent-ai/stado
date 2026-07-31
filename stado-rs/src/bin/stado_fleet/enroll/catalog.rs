//! The central enrollment and communication catalog.
//!
//! One optional top-level `enrollment` section of the canonical registry
//! declares which registration paths the fleet allows, and a `channels`
//! section declares how machines reach the control plane. Both are parsed
//! here, enforced in the preflights of `join`/`approve`/`enroll`, and
//! rendered by `stado_fleet catalog`. A document without the sections is
//! unrestricted — and says so out loud when the catalog is printed, so an
//! absent policy is never mistaken for a declared one.

use serde_json::Value;

/// The parsed `enrollment` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentCatalog {
    pub declared: bool,
    pub allow_join: bool,
    pub allow_enroll: bool,
    pub require_verified_hostname: bool,
}

/// The parsed `channels` section: declared channel names plus free-form
/// notes, preserving the operator's wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelsCatalog {
    pub declared: bool,
    pub control_plane: Vec<String>,
    pub notes: String,
}

fn bool_field(section: &Value, key: &str, location: &str) -> Result<bool, String> {
    match section.get(key) {
        None => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("{location}.{key}: must be a boolean")),
    }
}

/// Parse the optional `enrollment` section; undeclared means every path is
/// allowed and hostname verification is not required (today's behavior,
/// reported as unrestricted by `catalog`).
pub fn parse_enrollment(document: &Value) -> Result<EnrollmentCatalog, String> {
    let Some(section) = document.get("enrollment") else {
        return Ok(EnrollmentCatalog {
            declared: false,
            allow_join: true,
            allow_enroll: true,
            require_verified_hostname: false,
        });
    };
    if !section.is_object() {
        return Err("registry.enrollment: must be an object".to_string());
    }
    let location = "registry.enrollment";
    let allow_join = match section.get("allow_join") {
        None => true,
        Some(_) => bool_field(section, "allow_join", location)?,
    };
    let allow_enroll = match section.get("allow_enroll") {
        None => true,
        Some(_) => bool_field(section, "allow_enroll", location)?,
    };
    Ok(EnrollmentCatalog {
        declared: true,
        allow_join,
        allow_enroll,
        require_verified_hostname: bool_field(section, "require_verified_hostname", location)?,
    })
}

/// Parse the optional `channels` section.
pub fn parse_channels(document: &Value) -> Result<ChannelsCatalog, String> {
    let Some(section) = document.get("channels") else {
        return Ok(ChannelsCatalog {
            declared: false,
            control_plane: Vec::new(),
            notes: String::new(),
        });
    };
    let section = section
        .as_object()
        .ok_or_else(|| "registry.channels: must be an object".to_string())?;
    let mut control_plane = Vec::new();
    if let Some(value) = section.get("control_plane") {
        let entries = value
            .as_array()
            .ok_or_else(|| "registry.channels.control_plane: must be an array".to_string())?;
        for (index, entry) in entries.iter().enumerate() {
            let channel = entry.as_str().ok_or_else(|| {
                format!("registry.channels.control_plane[{index}]: must be a string")
            })?;
            control_plane.push(channel.to_string());
        }
    }
    let notes = section
        .get("notes")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(ChannelsCatalog {
        declared: true,
        control_plane,
        notes,
    })
}

/// Gate for machine-initiated registration.
pub fn require_join_allowed(document: &Value) -> Result<(), String> {
    let catalog = parse_enrollment(document)?;
    if !catalog.allow_join {
        return Err(
            "machine-initiated enrollment is disabled by registry.enrollment.allow_join"
                .to_string(),
        );
    }
    Ok(())
}

/// Gate for control-plane-initiated registration.
pub fn require_enroll_allowed(document: &Value) -> Result<(), String> {
    let catalog = parse_enrollment(document)?;
    if !catalog.allow_enroll {
        return Err(
            "control-plane enrollment is disabled by registry.enrollment.allow_enroll".to_string(),
        );
    }
    Ok(())
}

/// `stado_fleet catalog` — print the central catalog as declared in the
/// canonical registry.
pub async fn catalog(as_json: bool) -> Result<bool, String> {
    let document = stado::cli::registry::fetch_document()
        .await
        .map_err(|exc| exc.to_string())?;
    let enrollment = parse_enrollment(&document)?;
    let channels = parse_channels(&document)?;
    if as_json {
        let rendered = serde_json::json!({
            "enrollment": {
                "declared": enrollment.declared,
                "allow_join": enrollment.allow_join,
                "allow_enroll": enrollment.allow_enroll,
                "require_verified_hostname": enrollment.require_verified_hostname,
            },
            "channels": {
                "declared": channels.declared,
                "control_plane": channels.control_plane,
                "notes": channels.notes,
            },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&rendered).map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }
    if !enrollment.declared && !channels.declared {
        println!("no enrollment catalog declared; every registration path is unrestricted");
        return Ok(true);
    }
    println!("enrollment:");
    println!(
        "  allow_join={} allow_enroll={} require_verified_hostname={}",
        enrollment.allow_join, enrollment.allow_enroll, enrollment.require_verified_hostname
    );
    if channels.declared {
        println!("channels:");
        for channel in &channels.control_plane {
            println!("  control_plane: {channel}");
        }
        if !channels.notes.is_empty() {
            println!("  notes: {}", channels.notes);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn undeclared_catalog_is_unrestricted() {
        let catalog = parse_enrollment(&json!({})).expect("parse");
        assert!(!catalog.declared);
        assert!(catalog.allow_join);
        assert!(catalog.allow_enroll);
        assert!(!catalog.require_verified_hostname);
    }

    #[test]
    fn declared_catalog_disables_paths_explicitly() {
        let doc = json!({
            "enrollment": { "allow_join": false, "require_verified_hostname": true }
        });
        let catalog = parse_enrollment(&doc).expect("parse");
        assert!(catalog.declared);
        assert!(!catalog.allow_join);
        assert!(catalog.allow_enroll);
        assert!(catalog.require_verified_hostname);
        let err = require_join_allowed(&doc).unwrap_err();
        assert!(err.contains("allow_join"), "unexpected error: {err}");
        require_enroll_allowed(&doc).expect("enroll still allowed");
    }

    #[test]
    fn non_boolean_flag_is_refused() {
        let doc = json!({ "enrollment": { "allow_join": "yes" } });
        let err = parse_enrollment(&doc).unwrap_err();
        assert!(err.contains("must be a boolean"), "unexpected error: {err}");
    }

    #[test]
    fn channels_parse_entries_and_notes() {
        let doc = json!({
            "channels": {
                "control_plane": ["loopback", "tailnet"],
                "notes": "loopback only on the control plane host"
            }
        });
        let channels = parse_channels(&doc).expect("parse");
        assert!(channels.declared);
        assert_eq!(
            channels.control_plane,
            vec!["loopback".to_string(), "tailnet".to_string()]
        );
        assert_eq!(channels.notes, "loopback only on the control plane host");
    }

    #[test]
    fn malformed_channels_entry_is_refused() {
        let doc = json!({ "channels": { "control_plane": [true] } });
        let err = parse_channels(&doc).unwrap_err();
        assert!(err.contains("must be a string"), "unexpected error: {err}");
    }
}
