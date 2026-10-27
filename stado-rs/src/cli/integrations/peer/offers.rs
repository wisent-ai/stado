//! `stado market list|unlist|status|monitor --provider stado`: this host's
//! offer to a peer fleet, and what was lent.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::providers::peer::{declaration, loop_role, offer, remote, Offer};
use crate::providers::vast::{is_stado_busy, system_hostname};
use crate::queue::JobStorage;

use super::{cmd, our_store};

/// The refusal for an offer no loop on this host would act on.
fn no_loop(peer: &str) -> CmdError {
    CmdError::refused(format!(
        "no follow loop holds peer {peer} on this host, so an offer would lend nothing; \
         `stado market follow --provider stado --peer {peer} --idle-window-s SECONDS \
         --poll-interval-s SECONDS --price-gpu USD --max-duration-s SECONDS` lends this host \
         while our queue is idle"
    ))
}

/// Open this host to `peer` again at `price_gpu`, for the loop that holds
/// the peer here.
pub(super) async fn list(peer: &str, price_gpu: f64, json: bool) -> Result<(), CmdError> {
    if !price_gpu.is_finite() || price_gpu.is_sign_negative() {
        return Err(CmdError::usage(format!(
            "--price-gpu {price_gpu} is not a price; give US dollars per GPU-hour"
        )));
    }
    let store = our_store().await?;
    declaration::require(&store, peer).await.map_err(cmd)?;
    let home = crate::config_file::home().map_err(CmdError::declaration)?;
    let Some(holder) = crate::deploy::role_lock::holder(&home, &loop_role(peer))? else {
        return Err(no_loop(peer));
    };
    let host = system_hostname();
    let Some(mut current) = offer::load(&store, &host, peer).await.map_err(cmd)? else {
        return Err(no_loop(peer));
    };
    current.listed = true;
    current.price_gpu = price_gpu;
    current.changed_at = chrono::Utc::now().to_rfc3339();
    current.changed_by = "list".to_string();
    let stored = offer::save(&store, &current).await.map_err(cmd)?;
    answer(
        &stored,
        &format!(
            "{host} is offered to {peer} at ${price_gpu}/GPU-hour; the auto-list loop (pid {}) \
             lends it once our queue has been idle for its window",
            holder.pid
        ),
        json,
    )
}

/// Close this host to `peer`: the loop lends nothing more and drains a
/// running session at its next poll.
pub(super) async fn unlist(peer: &str, json: bool) -> Result<(), CmdError> {
    let store = our_store().await?;
    declaration::require(&store, peer).await.map_err(cmd)?;
    let host = system_hostname();
    let Some(mut current) = offer::load(&store, &host, peer).await.map_err(cmd)? else {
        return Err(CmdError::missing(format!(
            "{host} holds no offer for peer {peer}; nothing is listed to withdraw"
        )));
    };
    current.listed = false;
    current.changed_at = chrono::Utc::now().to_rfc3339();
    current.changed_by = "unlist".to_string();
    let stored = offer::save(&store, &current).await.map_err(cmd)?;
    let running = stored
        .session
        .as_ref()
        .is_some_and(|session| session.ended_at.is_none());
    let sentence = if running {
        format!(
            "{host} is withdrawn from {peer}; the auto-list loop drains the running lending \
             session at its next poll, and the peer's jobs it runs finish first"
        )
    } else {
        format!("{host} is withdrawn from {peer}; no lending session runs")
    };
    answer(&stored, &sentence, json)
}

fn answer(stored: &Offer, sentence: &str, json: bool) -> Result<(), CmdError> {
    if json {
        crate::cli::print_answer(&serde_json::to_value(stored)?, true)?;
    } else {
        println!("{sentence}");
    }
    Ok(())
}

/// With a peer: what this fleet's hosts lent it, from the peer's own job
/// records, at each host's offered price. Without one: what peer fleets ran
/// in our queue, from ours.
pub(super) async fn status(peer: Option<&str>, json: bool) -> Result<(), CmdError> {
    let store = our_store().await?;
    let Some(peer) = peer else {
        let ran = remote::lent_work(&store, None).await.map_err(cmd)?;
        return print(&json!({"ran_here_by_peers": ran}), json);
    };
    let declared = declaration::require(&store, peer).await.map_err(cmd)?;
    let offers = offer::for_peer(&store, peer).await.map_err(cmd)?;
    let peer_store = remote::open(&declared).await.map_err(cmd)?;
    let mut hosts = Vec::new();
    for offered in &offers {
        let lent = remote::lent_work(&peer_store, Some(&offered.host))
            .await
            .map_err(cmd)?;
        let seconds = match lent.get("seconds").and_then(Value::as_i64) {
            Some(seconds) => seconds,
            None => {
                return Err(CmdError::unreachable(format!(
                    "the lent-work reading for {} carries no seconds",
                    offered.host
                )))
            }
        };
        hosts.push(json!({
            "host": offered.host,
            "listed": offered.listed,
            "price_gpu": offered.price_gpu,
            "state": offered.state,
            "session": offered.session,
            "lent": lent,
            "amount_usd": remote::amount(seconds, offered.price_gpu),
        }));
    }
    print(&json!({"peer": peer, "hosts": hosts}), json)
}

/// One snapshot: our queue on this host beside the peer's queue and this
/// host's offer.
pub(super) async fn monitor(peer: &str, bucket: &str, json: bool) -> Result<(), CmdError> {
    let store = JobStorage::with_bucket(bucket).await.map_err(CmdError::from)?;
    let declared = declaration::require(&store, peer).await.map_err(cmd)?;
    let host = system_hostname();
    let ours = is_stado_busy(&store, &host).await.map_err(CmdError::from)?;
    let current = offer::load(&store, &host, peer).await.map_err(cmd)?;
    let peer_store = remote::open(&declared).await.map_err(cmd)?;
    let theirs = remote::queue_state(&peer_store).await.map_err(cmd)?;
    print(
        &json!({
            "now": chrono::Utc::now().to_rfc3339(),
            "hostname": host,
            "our_queue": {
                "queued": ours.queued,
                "running_here": ours.running_here,
                "idle": ours.idle,
            },
            "peer": peer,
            "peer_queue": theirs,
            "offer": current,
        }),
        json,
    )
}

fn print(value: &Value, json: bool) -> Result<(), CmdError> {
    crate::cli::print_answer(value, json)
}
