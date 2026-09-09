//! `stado fleet methods` — the ways a machine can be added, resolved against
//! one registry document and rendered as a table or as JSON.

use serde_json::Value;

use super::sections::parse_enrollment;

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
