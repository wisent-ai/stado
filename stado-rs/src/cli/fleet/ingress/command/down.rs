//! `stado fleet ingress down` — close the tunnel, stop the listener, and
//! unpublish the address.

use crate::cli::fleet::ingress::record::published;
use crate::cli::fleet::ingress::runtime::process::terminate_group;
use crate::cli::fleet::ingress::INGRESS_PATH;
use crate::queue::JobStorage;

/// `stado fleet ingress down` — close the tunnel, stop the listener, and
/// unpublish the address.
///
/// The tunnel goes first: closing the entrance before the thing behind it means
/// no request can arrive at a listener that is halfway through stopping. Both
/// are signalled as process *groups*, so nothing either of them spawned is left
/// holding the port.
///
/// The object is removed even when neither process was found. It records an
/// entrance that no longer exists, and leaving it behind would make `invite`
/// build a one-liner on a dead address — the exact failure this whole command
/// exists to prevent.
pub async fn down() -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let Some(ingress) = published(&store).await? else {
        println!("no ingress is published; nothing to stop");
        return Ok(true);
    };
    let this_machine = crate::providers::vast::system_hostname();
    if !ingress.pid_hint.machine.is_empty() && ingress.pid_hint.machine != this_machine {
        return Err(format!(
            "the published ingress runs on '{}', not on this machine ('{this_machine}'); its pids \
             mean nothing here and signalling them would hit something unrelated. Run \
             'stado fleet ingress down' there",
            ingress.pid_hint.machine
        ));
    }
    let tunnel_stopped = terminate_group(ingress.pid_hint.tunnel_pgid, "cloudflared");
    let listener_stopped = terminate_group(ingress.pid_hint.listener_pgid, "dashboard");
    store.delete_blob(INGRESS_PATH).await.map_err(|exc| {
        format!("both processes were stopped but {INGRESS_PATH} could not be removed: {exc}")
    })?;
    println!("ingress {} is down", ingress.base_url);
    println!(
        "  tunnel:   {}",
        if tunnel_stopped {
            format!("stopped (pgid {})", ingress.pid_hint.tunnel_pgid)
        } else {
            "was not running".to_string()
        }
    );
    println!(
        "  listener: {}",
        if listener_stopped {
            format!(
                "stopped (pgid {}, port {})",
                ingress.pid_hint.listener_pgid, ingress.listener_port
            )
        } else {
            "was not running".to_string()
        }
    );
    println!("  unpublished: {INGRESS_PATH}");
    println!("  every one-liner minted against that address stops working now.");
    Ok(true)
}
