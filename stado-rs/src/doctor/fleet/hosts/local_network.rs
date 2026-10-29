//! Whether macOS lets the Stado binary on this machine reach the local
//! network.

use crate::deploy::host_privacy::local_network::{running_binary, LocalNetwork};
use crate::doctor::Check;

pub(in crate::doctor) const LOCAL_NETWORK_ID: &str = "local-network";
pub(in crate::doctor) const LOCAL_NETWORK_TITLE: &str = "Local Network permission";
pub(in crate::doctor) const LOCAL_NETWORK_REMEDY: &str =
    "allow the binary in System Settings > Privacy & Security > Local Network";

/// What macOS last decided about Local Network for the binary this command
/// runs, since that binary was placed.
///
/// A Stado service launchd starts is not a command run from Terminal: macOS
/// asks before it reaches a LAN peer and keeps the answer per executable, so
/// a binary an install puts in place starts undecided, and a connect it is
/// refused answers "No route to host" while `nc` reaches the peer. The log is
/// read from the moment the binary was placed, so an older binary's answer
/// under the same code identifier does not stand in for this one's.
pub(in crate::doctor) async fn check_local_network() -> Check {
    let here = crate::providers::vast::system_hostname();
    let unmeasured = |detail: String| {
        Check::unmeasured(
            LOCAL_NETWORK_ID,
            LOCAL_NETWORK_TITLE,
            detail,
            LOCAL_NETWORK_REMEDY,
        )
    };
    if !cfg!(target_os = "macos") {
        return Check::pass(
            LOCAL_NETWORK_ID,
            LOCAL_NETWORK_TITLE,
            format!(
                "Local Network permission is macOS's; this machine runs {}",
                std::env::consts::OS
            ),
            LOCAL_NETWORK_REMEDY,
        )
        .measuring(LOCAL_NETWORK_ID, Some(here), 0);
    }
    let read = match running_binary(&crate::deploy::production_runner()).await {
        Ok(read) => read,
        Err(error) => return unmeasured(format!("not measured: {error}")),
    };
    let (program, start, decision) = (read.program, read.placed, read.decision);
    let named = match &decision.identifier {
        Some(identifier) => format!("{program} (code identifier {identifier})"),
        None => format!("{program} (no code identifier)"),
    };
    let at = decision.at.as_deref().unwrap_or("an unlogged time");
    let check = match decision.state {
        LocalNetwork::Granted => Check::pass(
            LOCAL_NETWORK_ID,
            LOCAL_NETWORK_TITLE,
            format!("macOS allowed {named} the local network at {at}"),
            LOCAL_NETWORK_REMEDY,
        ),
        LocalNetwork::Denied => Check::fail(
            LOCAL_NETWORK_ID,
            LOCAL_NETWORK_TITLE,
            format!(
                "macOS refused {named} the local network at {at}; every connection it opens to a \
                 LAN peer answers No route to host"
            ),
            LOCAL_NETWORK_REMEDY,
        ),
        LocalNetwork::Pending => Check::fail(
            LOCAL_NETWORK_ID,
            LOCAL_NETWORK_TITLE,
            format!(
                "macOS asked at the keyboard at {at} whether {named} may reach the local network and \
                 logged no answer; until it is answered its connections to LAN peers answer No route \
                 to host"
            ),
            LOCAL_NETWORK_REMEDY,
        ),
        LocalNetwork::NotMeasured => {
            return unmeasured(format!(
                "not measured: {named} reached nothing on the local network since it was placed at \
                 {start}"
            ))
            .measuring(LOCAL_NETWORK_ID, Some(here), 0)
        }
    };
    check.measuring(LOCAL_NETWORK_ID, Some(here), 1)
}
