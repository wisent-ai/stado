//! The central enrollment and communication catalog.
//!
//! One optional top-level `enrollment` section of the canonical registry
//! declares which registration paths the fleet allows, and a `channels`
//! section declares how machines reach the control plane. Both are parsed
//! here, enforced in the preflights of `join`/`approve`/`enroll`, and
//! rendered by `stado fleet catalog`. A document without the sections is
//! unrestricted — and says so out loud when the catalog is printed, so an
//! absent policy is never mistaken for a declared one.

use serde_json::Value;

/// The parsed `enrollment` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentCatalog {
    pub declared: bool,
    pub allow_join: bool,
    pub allow_enroll: bool,
    pub allow_invite: bool,
    pub allow_adopt: bool,
    pub require_verified_hostname: bool,
    pub key_custody: String,
}

/// Key custody values the fleet supports.
const CUSTODY_SKARBIEC: &str = "skarbiec";
const CUSTODY_OPENSSH: &str = "openssh";

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
            allow_invite: true,
            allow_adopt: true,
            require_verified_hostname: false,
            key_custody: CUSTODY_SKARBIEC.to_string(),
        });
    };
    if !section.is_object() {
        return Err("registry.enrollment: must be an object".to_string());
    }
    let location = "registry.enrollment";
    // Every allowance defaults to permitted: an `enrollment` section written
    // before a method existed must not silently forbid that method.
    let allowance = |key: &str| -> Result<bool, String> {
        match section.get(key) {
            None => Ok(true),
            Some(_) => bool_field(section, key, location),
        }
    };
    let key_custody = match section.get("key_custody") {
        None => CUSTODY_SKARBIEC.to_string(),
        Some(value) => {
            let custody = value
                .as_str()
                .ok_or_else(|| format!("{location}.key_custody: must be a string"))?;
            if custody != CUSTODY_SKARBIEC && custody != CUSTODY_OPENSSH {
                return Err(format!(
                    "{location}.key_custody: must be '{CUSTODY_SKARBIEC}' or '{CUSTODY_OPENSSH}'"
                ));
            }
            custody.to_string()
        }
    };
    Ok(EnrollmentCatalog {
        declared: true,
        allow_join: allowance("allow_join")?,
        allow_enroll: allowance("allow_enroll")?,
        allow_invite: allowance("allow_invite")?,
        allow_adopt: allowance("allow_adopt")?,
        require_verified_hostname: bool_field(section, "require_verified_hostname", location)?,
        key_custody,
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

/// Gate for invite-based registration: the operator mints a token, the
/// machine's owner runs one line.
pub fn require_invite_allowed(document: &Value) -> Result<(), String> {
    let catalog = parse_enrollment(document)?;
    if !catalog.allow_invite {
        return Err(
            "invite-based enrollment is disabled by registry.enrollment.allow_invite".to_string(),
        );
    }
    Ok(())
}

/// Gate for adoption: the operator already has an SSH session, so Stado
/// installs the fleet's public key itself.
pub fn require_adopt_allowed(document: &Value) -> Result<(), String> {
    let catalog = parse_enrollment(document)?;
    if !catalog.allow_adopt {
        return Err(
            "adoption is disabled by registry.enrollment.allow_adopt".to_string(),
        );
    }
    Ok(())
}

/// One way of adding a machine to the fleet, as reported by
/// `stado fleet methods`.
struct Method {
    name: &'static str,
    command: &'static str,
    summary: &'static str,
    requires: &'static str,
    provides: &'static str,
    /// Registry field that can switch the method off, or `None` for a method
    /// no catalog field gates.
    gate: Option<&'static str>,
    allowed: bool,
}

/// The fleet's four ways in, resolved against one registry document. This is
/// the single source of truth the CLI table, `--json`, the desktop app and the
/// public documentation all read; a method that exists and is not listed here
/// is a method nobody can discover.
fn methods_of(document: &Value) -> Result<Vec<Method>, String> {
    let enrollment = parse_enrollment(document)?;
    Ok(vec![
        Method {
            name: "invite",
            command: "stado fleet invite [--name NAME] [--offline]",
            summary: "send the machine's owner one line, or a fragment to paste when no control point answers",
            requires: "any channel to the machine's owner; the operator never needs to reach the machine. The one-line form also needs a control point serving /join.sh, which invite probes and says so when it does not",
            provides: "online: a single-use, expiring token whose one line installs the fleet's public key and files a pending request to approve. offline: no token and no route — a pasted fragment installs the same public key and prints the user@address the owner sends back, which the operator registers with 'stado fleet enroll NAME --ssh ADDRESS --bootstrap'",
            gate: Some("registry.enrollment.allow_invite"),
            allowed: enrollment.allow_invite,
        },
        Method {
            name: "adopt",
            command: "stado fleet enroll NAME --ssh DEST --install-key",
            summary: "operator can already open an SSH session, so Stado installs the key",
            requires: "an SSH session the operator can already open (password, agent, or an existing user key) plus write access to ~/.ssh on the machine",
            provides: "fleet-owned public key installed in authorized_keys, then the same probed, rollback-on-bootstrap-failure enroll as today",
            gate: Some("registry.enrollment.allow_adopt"),
            allowed: enrollment.allow_adopt,
        },
        Method {
            name: "join",
            command: "stado fleet join (on the machine), then stado fleet approve HOSTNAME",
            summary: "the machine announces itself; the operator approves",
            requires: "the stado binary and store credentials already present on the machine",
            provides: "a pending request filed by the machine itself, approved into a registered target",
            gate: Some("registry.enrollment.allow_join"),
            allowed: enrollment.allow_join,
        },
        Method {
            name: "declare",
            command: "stado registry host add NAME",
            summary: "declaration only, with no probe and no channel",
            requires: "nothing but a name",
            provides: "a registry entry with no channel and no proof of contact; the machine must bootstrap itself later",
            gate: None,
            allowed: true,
        },
    ])
}

/// `stado fleet methods` — the ways a machine can be added, and whether this
/// fleet's catalog allows each one.
pub async fn methods(as_json: bool) -> Result<bool, String> {
    let document = crate::cli::registry::fetch_document()
        .await
        .map_err(|exc| exc.to_string())?;
    let methods = methods_of(&document)?;
    if as_json {
        let rendered = serde_json::json!({
            "methods": methods
                .iter()
                .map(|method| serde_json::json!({
                    "name": method.name,
                    "command": method.command,
                    "summary": method.summary,
                    "requires": method.requires,
                    "provides": method.provides,
                    "allowed": method.allowed,
                    "gate": method.gate,
                }))
                .collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&rendered).map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }
    for method in &methods {
        println!(
            "{}\t{}",
            method.name,
            if method.allowed {
                "allowed"
            } else {
                "disabled by the registry catalog"
            }
        );
        println!("  command:  {}", method.command);
        println!("  requires: {}", method.requires);
        println!("  provides: {}", method.provides);
        println!(
            "  gate:     {}",
            method.gate.unwrap_or("none; always available")
        );
    }
    Ok(true)
}

/// `stado fleet catalog` — print the central catalog as declared in the
/// canonical registry.
pub async fn catalog(as_json: bool) -> Result<bool, String> {
    let document = crate::cli::registry::fetch_document()
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
                "allow_invite": enrollment.allow_invite,
                "allow_adopt": enrollment.allow_adopt,
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
        "  allow_join={} allow_enroll={} allow_invite={} allow_adopt={} require_verified_hostname={}",
        enrollment.allow_join,
        enrollment.allow_enroll,
        enrollment.allow_invite,
        enrollment.allow_adopt,
        enrollment.require_verified_hostname
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
    fn custody_defaults_to_the_vault() {
        let catalog = parse_enrollment(&json!({})).expect("parse");
        assert_eq!(catalog.key_custody, "skarbiec");
    }

    #[test]
    fn custody_accepts_openssh_alternative() {
        let doc = json!({ "enrollment": { "key_custody": "openssh" } });
        let catalog = parse_enrollment(&doc).expect("parse");
        assert_eq!(catalog.key_custody, "openssh");
    }

    #[test]
    fn unknown_custody_is_refused() {
        let doc = json!({ "enrollment": { "key_custody": "dropbox" } });
        let err = parse_enrollment(&doc).unwrap_err();
        assert!(err.contains("key_custody"), "unexpected error: {err}");
    }

    #[test]
    fn non_boolean_flag_is_refused() {
        let doc = json!({ "enrollment": { "allow_join": "yes" } });
        let err = parse_enrollment(&doc).unwrap_err();
        assert!(err.contains("must be a boolean"), "unexpected error: {err}");
    }

    #[test]
    fn allowances_absent_from_a_declared_section_stay_permitted() {
        let doc = json!({ "enrollment": { "allow_join": false } });
        let catalog = parse_enrollment(&doc).expect("parse");
        assert!(catalog.allow_invite);
        assert!(catalog.allow_adopt);
        require_invite_allowed(&doc).expect("invite still allowed");
        require_adopt_allowed(&doc).expect("adopt still allowed");
    }

    #[test]
    fn invite_and_adopt_are_gated_when_disabled() {
        let doc = json!({
            "enrollment": { "allow_invite": false, "allow_adopt": false }
        });
        let catalog = parse_enrollment(&doc).expect("parse");
        assert!(!catalog.allow_invite);
        assert!(!catalog.allow_adopt);
        let invite = require_invite_allowed(&doc).unwrap_err();
        assert!(invite.contains("allow_invite"), "unexpected error: {invite}");
        let adopt = require_adopt_allowed(&doc).unwrap_err();
        assert!(adopt.contains("allow_adopt"), "unexpected error: {adopt}");
    }

    #[test]
    fn methods_report_the_four_ways_with_their_gates() {
        let doc = json!({ "enrollment": { "allow_invite": false } });
        let methods = methods_of(&doc).expect("methods");
        let names: Vec<&str> = methods.iter().map(|method| method.name).collect();
        assert_eq!(names, vec!["invite", "adopt", "join", "declare"]);
        let invite = &methods[0];
        assert!(!invite.allowed);
        assert_eq!(invite.gate, Some("registry.enrollment.allow_invite"));
        let declare = &methods[3];
        assert!(declare.allowed);
        assert_eq!(declare.gate, None);
        assert!(methods
            .iter()
            .all(|method| !method.requires.is_empty() && !method.provides.is_empty()));
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
