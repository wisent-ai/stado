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
             ({}); create one with `{PROVISION_HINT}`, or record a host Stado did not create \
             with `{DECLARE_HINT}`",
            problems.join("; ")
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })
}

/// The two commands that produce an edge, with every flag each requires.
const PROVISION_HINT: &str = "stado web edge provision <name> --region <region> --size <size> \
                              --image <urn> --disk-gb <GiB> --contact <mail> --probe-seconds <s>";
const DECLARE_HINT: &str = "stado web edge declare --target <registered host> --address <ipv4> \
                            --contact <mail> --probe-seconds <s>";

/// The sentence for an edge declared on `target`, a name the canonical
/// registry does not hold. One sentence for every command that refuses it:
/// `stado config validate`, `stado web edge status`, `stado web route`,
/// `stado web edge hostnames` and `stado web origin declare --publication
/// web-edge`.
pub(crate) fn unregistered_refusal(target: &str) -> String {
    format!(
        "the public edge is declared on {target}, which the canonical registry does not hold, so \
         no hostname can be published through it; create the edge with `{PROVISION_HINT}`, or \
         register the host and record it with `{DECLARE_HINT}`"
    )
}

/// The declared edge, refused when the canonical registry does not hold its
/// host: every hostname `stado web route` publishes is delivered to that
/// host's Stado, so an edge declared on a name the fleet does not hold fails
/// only after the plan was made, naming the host and nothing about the
/// declaration that caused it.
///
/// A registry nobody can read is that failure, not this one; a name the
/// registry does not hold is refused with the two commands that fix it.
pub(in crate::cli::web) async fn registered() -> Result<&'static WebApiEdge, CmdError> {
    let edge = declared()?;
    crate::cli::canonical_host(edge.target())
        .await
        .map_err(|error| match error.failure {
            Some(crate::primitives::failure::FailureCode::NotFound) => {
                CmdError::click(unregistered_refusal(edge.target()))
                    .stating(crate::primitives::failure::FailureCode::Config)
            }
            _ => error,
        })?;
    Ok(edge)
}

/// Record the edge in the configuration.
///
/// `map.clear()` first: the plane's parser refuses an unsupported key, so a
/// leftover one from an earlier shape would make every later read of the
/// section fail rather than be ignored. The registrar credential is the one
/// field carried over when the caller names none, because re-provisioning a
/// host does not change which registrar item writes its records.
pub(super) fn record(
    target: &str,
    address: &str,
    contact: &str,
    registrar_credential: Option<&str>,
    probe_seconds: Option<u64>,
) -> Result<(&'static str, Option<String>), CmdError> {
    let existing = crate::config_file::get("web_api.edge");
    let existed = existing.is_some();
    let registrar_credential = registrar_credential.map(str::to_string).or_else(|| {
        existing
            .as_ref()
            .and_then(|edge| edge.get("registrar_credential"))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    });
    // The probe deadline is carried over like the credential: declaring the
    // same edge again does not change how long its ports are waited for.
    let probe_seconds = probe_seconds.or_else(|| {
        existing
            .as_ref()
            .and_then(|edge| edge.get("probe_seconds"))
            .and_then(|value| value.as_u64())
    });
    let target = target.to_string();
    let address = address.to_string();
    let contact = contact.to_string();
    super::mutate_web("edge", |map| {
        map.clear();
        map.insert("target".to_string(), json!(target));
        map.insert("address".to_string(), json!(address));
        map.insert("contact".to_string(), json!(contact));
        if let Some(item) = &registrar_credential {
            map.insert("registrar_credential".to_string(), json!(item));
        }
        if let Some(seconds) = probe_seconds {
            map.insert("probe_seconds".to_string(), json!(seconds));
        }
        Ok(())
    })?;
    let change = if existed { "replaced" } else { "declared" };
    Ok((change, registrar_credential))
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
    registrar_credential: Option<&str>,
    probe_seconds: Option<u64>,
    json_output: bool,
) -> Result<(), CmdError> {
    checked_declaration(target, contact)?;
    if address.parse::<std::net::Ipv4Addr>().is_err() {
        return Err(CmdError::usage(format!(
            "--address {address:?} must be the edge's public IPv4 address, because a product \
             hostname's A record is written to point at it"
        )));
    }
    if registrar_credential.is_some_and(|item| item.trim().is_empty()) {
        return Err(CmdError::usage(
            "--registrar-credential must name a Skarbiec item".to_string(),
        ));
    }
    checked_probe(probe_seconds)?;
    let (change, registrar_credential) = record(
        target,
        address,
        contact,
        registrar_credential,
        probe_seconds,
    )?;
    let report = json!({
        "target": target,
        "address": address,
        "contact": contact,
        "registrar_credential": registrar_credential,
        "change": change,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{target} at {address}: {change} as the fleet's web edge");
    }
    Ok(())
}

/// A probe deadline of zero would report every port unanswered at once.
pub(super) fn checked_probe(probe_seconds: Option<u64>) -> Result<(), CmdError> {
    if probe_seconds == Some(0) {
        return Err(CmdError::usage(
            "--probe-seconds must be above zero: it is how long edge status waits for 80 and 443 \
             to answer from here",
        ));
    }
    Ok(())
}
