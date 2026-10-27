//! Working out an address for one service, and proving something is there.
//!
//! The three pieces every verb below shares: where a host is reachable, which
//! port a service listens on, and whether anything answers at the address the
//! directory produced.

use serde_json::Value;

use crate::targets;

pub(crate) mod assigned;
pub(in crate::cli::directory) mod connect;
pub(in crate::cli::directory) mod consumers;
pub(in crate::cli::directory) mod endpoints;

/// The address a host is reachable at from off-box, taken from its own record.
///
/// `ssh` carries `user@address` for the channel Stado already trusts, so its
/// address half is the one this fleet has agreed on. A declared hostname is
/// accepted after it, for hosts reached by name rather than by number.
pub(crate) fn routable_address(target: &targets::ComputeTarget) -> Option<String> {
    let (_, ssh) = target.ssh_connections().next()?;
    // `user@address`, or a bare address when the record names no user.
    let address = match ssh.rsplit_once('@') {
        Some((_user, address)) => address.trim(),
        None => ssh.trim(),
    };
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
        // A declared port is the answer; one wider than a TCP port names
        // nothing this fleet can dial.
        return match u16::try_from(port) {
            Ok(port) => Some(port),
            Err(_wider_than_a_port) => None,
        };
    }
    let declared = entry
        .get("endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(active))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str)?;
    // The port is the one the URL states; a URL that states none, or text
    // that is not a URL, names no port.
    match url::Url::parse(declared) {
        Ok(url) => url.port(),
        Err(_not_a_url) => None,
    }
}

/// Prove something answers HTTP there. A gateway that refuses an
/// unauthenticated caller has still answered, so any status counts; what does
/// not count is a socket that accepts and says nothing, which is what a stale
/// forward looks like from the outside. An adapter this host's resolver
/// already holds a connection on, unanswered past the directory refresh
/// interval, is not knocked on: the knock would stand behind that
/// connection for as long as it stands, and the resolver's own record of it
/// is the answer.
async fn answers(url: &str) -> Result<u16, String> {
    let parsed = url::Url::parse(url).map_err(|error| format!("{url}: {error}"))?;
    match crate::cli::resolver::held_at(&parsed) {
        Ok(None) => {}
        Ok(Some(held)) => return Err(held),
        Err(unreadable) => {
            return Err(format!(
                "whether this host's resolver holds that adapter cannot be read: {unreadable}"
            ))
        }
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|error| crate::cli::http_failure(&error))?;
    // The whole cause chain: reqwest's own sentence is "error sending request
    // for url", and whether the port refused, the host was unreachable or the
    // peer reset is one layer down.
    crate::wait::request(client.get(url))
        .await
        .map(|response| response.status().as_u16())
        .map_err(|error| crate::cli::http_failure(&error))
}
