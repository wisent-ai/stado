//! The declaration: the edge the configuration holds, the two shapes a
//! proposed one has to have, and recording it.

use serde_json::json;

use super::CmdError;
use crate::config::{self, WebApiEdge};

/// The declared edge, or the sentence that says what produces one.
///
/// Public to the module because `stado web route` needs exactly this refusal:
/// a product whose edge is `stado` cannot be published at all until one host
/// holds an address, and "no edge is declared" is a different problem from
/// every other way routing fails.
pub(in crate::cli::web) fn declared() -> Result<&'static WebApiEdge, CmdError> {
    config::web_api_edge().map_err(|problems| {
        CmdError::click(format!(
            "no public edge is declared, so nothing can terminate TLS for a fleet hostname \
             ({}); create one with `stado web edge provision <name> --contact <mail>`, or \
             record a host Stado did not create with `stado web edge declare --target <host> \
             --address <ipv4> --contact <mail>`",
            problems.join("; ")
        ))
    })
}

/// Record the edge in the configuration.
///
/// `map.clear()` first: the plane's parser refuses an unsupported key, so a
/// leftover one from an earlier shape would make every later read of the
/// section fail rather than be ignored.
pub(super) fn record(target: &str, address: &str, contact: &str) -> Result<&'static str, CmdError> {
    let existed = crate::config_file::get("web_api.edge").is_some();
    let target = target.to_string();
    let address = address.to_string();
    let contact = contact.to_string();
    super::mutate_web("edge", |map| {
        map.clear();
        map.insert("target".to_string(), json!(target));
        map.insert("address".to_string(), json!(address));
        map.insert("contact".to_string(), json!(contact));
        Ok(())
    })?;
    Ok(if existed { "replaced" } else { "declared" })
}

/// The name and contact checks that happen before Azure is touched.
///
/// The configuration plane enforces the same two shapes when the result is
/// written. Checking them here is the difference between refusing a typo and
/// refusing it after creating a VM the configuration will not accept.
pub(super) fn checked_declaration(target: &str, contact: &str) -> Result<(), CmdError> {
    let canonical = !target.is_empty()
        && target
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !canonical {
        return Err(CmdError::usage(format!(
            "{target:?} is not a canonical target name; use lowercase letters, digits and dashes"
        )));
    }
    if !contact.contains('@') || contact.chars().any(char::is_whitespace) {
        return Err(CmdError::usage(format!(
            "--contact {contact:?} must be the mail address Let's Encrypt sends expiry warnings to"
        )));
    }
    Ok(())
}

pub(super) fn declare(
    target: &str,
    address: &str,
    contact: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    checked_declaration(target, contact)?;
    if address.parse::<std::net::Ipv4Addr>().is_err() {
        return Err(CmdError::usage(format!(
            "--address {address:?} must be the edge's public IPv4 address, because a product \
             hostname's A record is written to point at it"
        )));
    }
    let change = record(target, address, contact)?;
    let report = json!({
        "target": target,
        "address": address,
        "contact": contact,
        "change": change,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{target} at {address}: {change} as the fleet's web edge");
    }
    Ok(())
}
