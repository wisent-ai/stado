//! Working out an address for one service, and proving something is there.
//!
//! The three pieces every verb below shares: where a host is reachable, which
//! port a service listens on, and whether anything answers at the address the
//! directory produced.

use serde_json::Value;

use crate::targets;

pub(in crate::cli::directory) mod connect;
pub(in crate::cli::directory) mod consumers;
pub(in crate::cli::directory) mod endpoints;

/// The address a host is reachable at from off-box, taken from its own record.
///
/// `ssh` carries `user@address` for the channel Stado already trusts, so its
/// address half is the one this fleet has agreed on. A declared hostname is
/// accepted after it, for hosts reached by name rather than by number.
fn routable_address(target: &targets::ComputeTarget) -> Option<String> {
    let (_, ssh) = target.ssh_connections().next()?;
    let address = ssh.rsplit('@').next().unwrap_or(ssh).trim();
    (!address.is_empty()).then(|| address.to_string())
}

/// The port the service listens on.
///
/// `port` on the service record is the answer. Until every record carries one,
/// the port is read back out of the address declared for the placed host --
/// that address is the one written by whoever started the service, so its port
/// is a fact even while the address around it is not.
pub(crate) fn service_port(entry: &Value, active: &str) -> Option<u16> {
    if let Some(port) = entry.get("port").and_then(Value::as_u64) {
        return u16::try_from(port).ok();
    }
    let declared = entry
        .get("endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(active))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str)?;
    declared
        .rsplit(':')
        .next()
        .and_then(|tail| tail.trim_end_matches('/').parse().ok())
}

/// Prove something answers HTTP there. A gateway that refuses an
/// unauthenticated caller has still answered, so any status counts; what does
/// not count is a socket that accepts and says nothing, which is what a stale
/// forward looks like from the outside.
async fn answers(url: &str) -> Result<u16, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            "5".parse().expect("static number"),
        ))
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())?;
    client
        .get(url)
        .send()
        .await
        .map(|response| response.status().as_u16())
        .map_err(|error| error.to_string())
}
