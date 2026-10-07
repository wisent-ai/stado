//! `stado fleet ingress down` — close the tunnel, stop the listener, and
//! unpublish the address.

use crate::cli::fleet::ingress::record::published;
use crate::cli::fleet::ingress::runtime::process::terminate_group;
use crate::cli::fleet::ingress::INGRESS_PATH;
use crate::cli::CmdError;
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
pub async fn down(as_json: bool) -> Result<bool, CmdError> {
    let store = JobStorage::new().await?;
    let Some(ingress) = published(&store).await? else {
        if as_json {
            let answer = serde_json::json!({ "published": false, "stopped": false });
            crate::cli::print_answer(&answer, true)?;
        } else {
            println!("no ingress is published; nothing to stop");
        }
        return Ok(true);
    };
    let this_machine = crate::providers::vast::system_hostname();
    if !ingress.pid_hint.machine.is_empty() && ingress.pid_hint.machine != this_machine {
        return Err(CmdError::refused(format!(
            "the published ingress runs on '{}', not on this machine ('{this_machine}'); its pids \
             mean nothing here and signalling them would hit something unrelated. Run \
             'stado fleet ingress down' there",
            ingress.pid_hint.machine
        )));
    }
    let tunnel_stopped = terminate_group(ingress.pid_hint.tunnel_pgid, "cloudflared")?;
    let listener_stopped = terminate_group(ingress.pid_hint.listener_pgid, "--enrollment-only")?;
    store.delete_blob(INGRESS_PATH).await.map_err(|exc| {
        let cause = CmdError::from(exc);
        let mut error = CmdError::click(format!(
            "both processes were stopped but {INGRESS_PATH} could not be removed: {cause}"
        ));
        error.failure = cause.failure;
        error
    })?;
    if as_json {
        let answer = serde_json::json!({
            "published": true,
            "stopped": true,
            "base_url": ingress.base_url,
            "tunnel": { "pgid": ingress.pid_hint.tunnel_pgid, "signalled": tunnel_stopped },
            "listener": {
                "pgid": ingress.pid_hint.listener_pgid,
                "port": ingress.listener_port,
                "signalled": listener_stopped,
            },
            "unpublished": INGRESS_PATH,
        });
        crate::cli::print_answer(&answer, true)?;
        return Ok(true);
    }
    println!("ingress {} is down", ingress.base_url);
    println!(
        "  tunnel:   {}",
        if tunnel_stopped {
            format!("signalled (pgid {})", ingress.pid_hint.tunnel_pgid)
        } else {
            "was not running".to_string()
        }
    );
    println!(
        "  listener: {}",
        if listener_stopped {
            format!(
                "signalled (pgid {}, port {})",
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
