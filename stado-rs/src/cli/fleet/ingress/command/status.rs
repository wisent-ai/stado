//! `stado fleet ingress status` — what is published, whether it still answers,
//! and how old it is.

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::cli::fleet::ingress::record::published;
use crate::cli::fleet::ingress::runtime::process::group_alive;
use crate::cli::fleet::ingress::MODE_QUICK;
use crate::queue::JobStorage;

/// Seconds between an RFC 3339 stamp and now, when the stamp parses.
fn age_seconds(stamp: &str, now: DateTime<Utc>) -> Option<i64> {
    DateTime::parse_from_rfc3339(stamp)
        .ok()
        .map(|parsed| (now - parsed.with_timezone(&Utc)).num_seconds())
}

/// `stado fleet ingress status [--json]` — what is published, whether it still
/// answers, and how old it is.
///
/// The address is probed *now* rather than reported from the stored
/// `verified_at`: a published object proves the entrance worked when it was
/// stood up, and the only question worth asking later is whether it still does.
pub async fn status(as_json: bool) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let Some(ingress) = published(&store).await? else {
        if as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "published": false,
                    "detail": "no ingress is published; 'stado fleet ingress up' stands one up",
                }))
                .map_err(|exc| exc.to_string())?
            );
        } else {
            println!("no ingress is published");
            println!("  stand one up: stado fleet ingress up");
        }
        return Ok(true);
    };

    let now = Utc::now();
    let this_machine = crate::providers::vast::system_hostname();
    let local = ingress.pid_hint.machine.is_empty() || ingress.pid_hint.machine == this_machine;
    let listener_alive = local && group_alive(ingress.pid_hint.listener_pgid, "dashboard");
    let tunnel_alive = local && group_alive(ingress.pid_hint.tunnel_pgid, "cloudflared");
    let checkpoint = crate::cli::fleet::invite::probe_checkpoint(&ingress.base_url).await;
    let standing = age_seconds(&ingress.started_at, now);
    let since_verified = age_seconds(&ingress.verified_at, now);

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "published": true,
                "base_url": ingress.base_url,
                "mode": ingress.mode,
                "host": ingress.host,
                "started_at": ingress.started_at,
                "verified_at": ingress.verified_at,
                "standing_seconds": standing,
                "seconds_since_verified": since_verified,
                "listener_port": ingress.listener_port,
                "reachable": checkpoint.reachable,
                "reason": checkpoint.reason,
                "detail": checkpoint.detail,
                "processes_on_this_machine": local,
                "listener_alive": listener_alive,
                "tunnel_alive": tunnel_alive,
                "pid_hint": {
                    "machine": ingress.pid_hint.machine,
                    "listener_pgid": ingress.pid_hint.listener_pgid,
                    "tunnel_pgid": ingress.pid_hint.tunnel_pgid,
                    "listener_log": ingress.pid_hint.listener_log,
                    "tunnel_log": ingress.pid_hint.tunnel_log,
                },
                "temporary": ingress.mode == MODE_QUICK,
            }))
            .map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }

    println!("ingress {} (mode: {})", ingress.base_url, ingress.mode);
    match standing {
        Some(seconds) => println!("  standing:  {seconds}s (since {})", ingress.started_at),
        None => println!("  standing:  unknown (started_at: {})", ingress.started_at),
    }
    match since_verified {
        Some(seconds) => println!(
            "  verified:  {} ({seconds}s ago, from the internet)",
            ingress.verified_at
        ),
        None => println!("  verified:  {}", ingress.verified_at),
    }
    println!(
        "  listener:  \
         127.0.0.1:{} ({})",
        ingress.listener_port,
        if listener_alive {
            "running"
        } else {
            "not running"
        }
    );
    println!(
        "  tunnel:    {}",
        if tunnel_alive {
            "running"
        } else {
            "not running"
        }
    );
    if !local {
        println!(
            "  the two processes belong to '{}', not to this machine, so their state is unknown here",
            ingress.pid_hint.machine
        );
    }
    println!("  answering: {}", checkpoint.detail);
    if ingress.mode == MODE_QUICK {
        println!(
            "  this is a quick tunnel: not a production entrance, rate limited, and its address"
        );
        println!("  changes on every restart.");
    }
    Ok(true)
}
