//! What a refused connection to a peer on this machine's LAN says on macOS.
//!
//! macOS asks the person at the machine before a program reaches the local
//! network, and keeps the answer per executable, so a binary an install puts
//! in place starts undecided. A program without the permission is not told
//! so: its connect to a LAN address fails with EHOSTUNREACH, "No route to
//! host", the same answer a peer that is off or asleep gives once ARP finds
//! nobody, while `nc` and `ssh` from the same shell carry their own
//! permission. So a host-unreachable answer from a LAN address is read
//! against what macOS logged for this binary since it was placed, and named
//! as the denial, the unanswered question, a peer that did not answer although
//! the binary is allowed, or a binary macOS logged nothing for.
//! <https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy>

use std::io;
use std::net::{IpAddr, SocketAddr};

use crate::deploy::host_privacy::local_network::{running_binary, LocalNetwork};

/// Where the person at the Mac gives a program the local network.
const SETTINGS: &str = "System Settings > Privacy & Security > Local Network";

/// Whether `address` is on a local network: a private or link-local address,
/// the ones macOS's Local Network permission governs.
fn local(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

/// What `error`, a failed connect to `addresses`, means on macOS when it is
/// host-unreachable from a LAN address, by what macOS decided for this
/// binary; none for any other failure.
pub(crate) async fn local_network(error: &io::Error, addresses: &[SocketAddr]) -> Option<String> {
    if !cfg!(target_os = "macos") || error.kind() != io::ErrorKind::HostUnreachable {
        return None;
    }
    let peer = addresses.iter().find(|address| local(&address.ip()))?;
    let lan = format!("{peer} is on this machine's local network");
    // The decision below is read for the file at this process's path, which
    // an install may have replaced since the process started: a network node
    // left on the replaced image was refused every LAN peer for days while
    // the installed file, and every fresh process, reached them, and this
    // answer blamed the peer.
    match replaced_program() {
        Ok(Some(program)) => {
            return Some(format!(
                "{lan}, and this process still executes a Stado image an install has since \
                 replaced at {program}: macOS keeps its Local Network answer per executable, and a \
                 process left on a replaced image is refused the local network while a fresh \
                 process of the installed file reaches {peer}; `stado release local converge` \
                 restarts every unit still executing a replaced image"
            ))
        }
        Ok(None) => {}
        Err(cause) => {
            return Some(format!(
                "{lan}, and whether this process still executes the file it started from cannot \
                 be read ({cause}); a process left on a replaced image is refused the local network, \
                 and a peer that is off or asleep answers the same: nc reaching {peer} from the \
                 same shell tells the two apart"
            ))
        }
    }
    let read = match running_binary(&crate::deploy::production_runner()).await {
        Ok(read) => read,
        Err(cause) => {
            return Some(format!(
                "{lan}, and whether macOS allows this binary the local network cannot be read \
                 ({cause}); a peer that is off or asleep answers the same, and nc reaching {peer} \
                 from the same shell tells the two apart"
            ))
        }
    };
    let named = match &read.decision.identifier {
        Some(identifier) => format!("{} (code identifier {identifier})", read.program),
        None => format!("{} (no code identifier)", read.program),
    };
    let at = read.decision.at.as_deref().unwrap_or("an unlogged time");
    Some(match read.decision.state {
        LocalNetwork::Granted => format!(
            "{lan}, and macOS allowed {named} the local network at {at}, so {peer} itself did not \
             answer: it is off, asleep or no longer at that address"
        ),
        LocalNetwork::Denied => format!(
            "{lan}, and macOS refused {named} the local network at {at}: it needs Local Network \
             access in {SETTINGS}"
        ),
        LocalNetwork::Pending => format!(
            "{lan}, and macOS asked at the keyboard at {at} whether {named} may reach the local \
             network and logged no answer: it needs Local Network access in {SETTINGS}"
        ),
        LocalNetwork::NotMeasured => format!(
            "{lan}, and macOS logged no Local Network decision for {named} since it was placed at \
             {}: if nc reaches {peer} from the same shell, macOS is refusing this binary and it \
             needs Local Network access in {SETTINGS}; if nc does not, {peer} itself is off, asleep \
             or gone",
            read.placed
        ),
    })
}

/// The path of this process's program when the file there is no longer the
/// image this process executes (an install placed another one since it
/// started); none while it still is.
fn replaced_program() -> Result<Option<String>, String> {
    let program = std::env::current_exe()
        .map_err(|error| format!("which binary this is cannot be read: {error}"))?;
    let pid = std::process::id();
    let running = crate::deploy::service::running_images(&[pid])?
        .remove(&pid)
        .ok_or_else(|| format!("the image pid {pid} executes could not be read"))?;
    let (installed, _) = crate::deploy::service::installed_image(&program)
        .map_err(|error| format!("{} cannot be read: {error}", program.display()))?;
    Ok((!running.is_same_file(&installed)).then(|| program.display().to_string()))
}
