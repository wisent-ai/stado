//! `stado fleet ingress status` — what is published, whether it still answers,
//! and how old it is.

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::cli::fleet::ingress::record::published;
use crate::cli::fleet::ingress::runtime::process::group_alive;
use crate::cli::fleet::ingress::MODE_QUICK;
use crate::cli::CmdError;
use crate::queue::JobStorage;

/// Seconds between an RFC 3339 stamp and now, or why the stamp names no time.
fn age_seconds(stamp: &str, now: DateTime<Utc>) -> Result<i64, String> {
    DateTime::parse_from_rfc3339(stamp)
        .map(|parsed| (now - parsed.with_timezone(&Utc)).num_seconds())
        .map_err(|error| format!("'{stamp}' is not an RFC 3339 time: {error}"))
}

/// An age in the JSON answer: its seconds, or null when the stamp names no
/// time, whose reason `stamp_errors` carries beside it.
fn age_json(age: &Result<i64, String>) -> serde_json::Value {
    match age {
        Ok(seconds) => json!(seconds),
        Err(_reason_in_stamp_errors) => serde_json::Value::Null,
    }
}

/// How a process group reads in the human answer: its state on this machine,
/// or unknown when the group belongs to another one.
fn process_state(alive: Option<bool>) -> &'static str {
    match alive {
        Some(true) => "running",
        Some(false) => "not running",
        None => "unknown on this machine",
    }
}

/// `stado fleet ingress status [--json]` — what is published, whether it still
/// answers, and how old it is.
///
/// The address is probed *now* rather than reported from the stored
/// `verified_at`: a published object proves the entrance worked when it was
/// stood up, and the only question worth asking later is whether it still does.
pub async fn status(as_json: bool) -> Result<bool, CmdError> {
    let store = JobStorage::new().await?;
    let Some(ingress) = published(&store).await? else {
        if as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "published": false,
                    "detail": "no ingress is published; 'stado fleet ingress up' stands one up",
                }))?
            );
        } else {
            println!("no ingress is published");
            println!("  stand one up: stado fleet ingress up");
        }
        return Ok(true);
    };

    let now = Utc::now();
    let this_machine = crate::providers::vast::system_hostname();
    // parse_ingress refuses an object that names no machine, so a group whose
    // machine is another host's is never read as running or stopped here.
    let local = ingress.pid_hint.machine == this_machine;
    let listener_alive = match local {
        true => Some(group_alive(
            ingress.pid_hint.listener_pgid,
            "--enrollment-only",
        )?),
        false => None,
    };
    let tunnel_alive = match local {
        true => Some(group_alive(ingress.pid_hint.tunnel_pgid, "cloudflared")?),
        false => None,
    };
    let checkpoint = crate::cli::fleet::invite::probe_checkpoint(&ingress.base_url).await;
    let standing = age_seconds(&ingress.started_at, now);
    let since_verified = age_seconds(&ingress.verified_at, now);
    let stamp_errors: Vec<String> = [("started_at", &standing), ("verified_at", &since_verified)]
        .into_iter()
        .filter_map(|(key, age)| age.as_ref().err().map(|why| format!("{key} {why}")))
        .collect();

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
                "standing_seconds": age_json(&standing),
                "seconds_since_verified": age_json(&since_verified),
                "stamp_errors": stamp_errors,
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
            }))?
        );
        return Ok(true);
    }

    println!("ingress {} (mode: {})", ingress.base_url, ingress.mode);
    match standing {
        Ok(seconds) => println!("  standing:  {seconds}s (since {})", ingress.started_at),
        Err(why) => println!("  standing:  unknown: started_at {why}"),
    }
    match since_verified {
        Ok(seconds) => println!(
            "  verified:  {} ({seconds}s ago, from the internet)",
            ingress.verified_at
        ),
        Err(why) => println!("  verified:  unknown: verified_at {why}"),
    }
    println!(
        "  listener:  {}:{} ({})",
        std::net::Ipv4Addr::LOCALHOST,
        ingress.listener_port,
        process_state(listener_alive)
    );
    println!("  tunnel:    {}", process_state(tunnel_alive));
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
