//! The public addresses the fleet's own hosts reach the Internet from: what
//! a cloud host admits to SSH, so that nobody else reaches it.
//!
//! Each host named by `cloud_hosts.admit_from` is asked over its host
//! channel, and asks the product site (`<api.url>/api/egress`), which answers
//! with the address its request arrived from. The answers are recorded in the
//! registry as `cloud_hosts.egress`, with when they were read.

use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::{host_channel, shlex_quote};

/// The product site route that answers a caller's own public address.
const EGRESS_ROUTE: &str = "/api/egress";

/// `(host, address)` for every host `admit_from` names, each read now.
pub(super) async fn measure(admit_from: &[String]) -> Result<Vec<(String, String)>, CmdError> {
    let origin = crate::config::stado_api_url();
    if origin.is_empty() {
        return Err(CmdError::declaration(
            "api.url (STADO_API_URL) is not declared, so no host can be asked for its public address; \
             `stado config set api.url <origin>` declares the product site",
        ));
    }
    let url = format!("{origin}{EGRESS_ROUTE}");
    let script = format!("exec curl -4 -fsS {}\n", shlex_quote(&url));
    let runner = crate::deploy::production_runner();
    let mut measured = Vec::new();
    for host in admit_from {
        let target = host_channel::canonical_target(host).await?;
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(|error| {
                CmdError::from(error)
                    .within(format!("{host} could not be asked for its public address"))
            })?;
        if !output.succeeded() {
            return Err(CmdError::unreachable(format!(
                "{host} asked {url} for its public address and curl exited {}: {}",
                output.code,
                output.detail().trim()
            )));
        }
        let answer: Value = serde_json::from_str(output.stdout.trim()).map_err(|error| {
            CmdError::unreachable(format!(
                "{url} answered {host} with {:?}, which is not JSON: {error}",
                output.stdout.trim()
            ))
        })?;
        let address = answer
            .get("address")
            .and_then(Value::as_str)
            .filter(|address| address.parse::<std::net::Ipv4Addr>().is_ok())
            .ok_or_else(|| {
                CmdError::unreachable(format!(
                    "{url} answered {host} with {answer}, which names no IPv4 `address`"
                ))
            })?;
        measured.push((host.clone(), address.to_string()));
    }
    Ok(measured)
}

/// The distinct addresses of `measured`, as a security rule admits them.
pub(super) fn sources(measured: &[(String, String)]) -> Vec<String> {
    let mut sources: Vec<String> = measured
        .iter()
        .map(|(_, address)| address.clone())
        .collect();
    sources.sort();
    sources.dedup();
    sources
}
