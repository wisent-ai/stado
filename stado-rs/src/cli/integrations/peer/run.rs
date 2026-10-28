//! `stado market follow --provider stado --peer NAME`: lend this host to
//! a peer fleet while our own queue is idle.
//!
//! Every poll reads our queue the way the Vast bridge does
//! ([`crate::providers::vast::is_stado_busy`]): idle means nothing is queued
//! and no running job names this host. After `--idle-window-s` idle seconds,
//! with the offer listed and the peer's queue holding work, a lending worker
//! starts ([`crate::providers::peer::lending`]). The moment our work
//! appears, the operator unlists, an inference reservation holds the cards,
//! or the session has run `--max-duration-s`, the worker is asked to drain:
//! it claims nothing more and exits once the peer's jobs it runs have
//! finished. A worker that ends by itself — the peer's queue held nothing
//! more it could run — ends the session, and the next idle poll may start
//! another.
//!
//! The polls come at the operator's `--poll-interval-s`, the same cadence
//! the Vast bridge keeps; nothing in the loop chooses a period of its own.
//!
//! The loop holds the role lock `market-stado--<peer>` for its life, so one
//! loop lends this host to one peer and `stado market list` can tell that a
//! loop is there. Every transition is written to this host's offer record,
//! which the CLI and Stado Desktop read.

mod listing;

use std::time::{Duration, Instant};

use crate::cli::CmdError;
use crate::providers::peer::lending::Lending;
use crate::providers::peer::{declaration, loop_role, offer, remote, Offer, PeerDeclaration};
use crate::providers::vast::{is_stado_busy, system_hostname};
use crate::queue::JobStorage;

use self::listing::{record, start_offer};
use super::{cmd, our_store};

/// The loop's arguments, as the command took them.
pub(super) struct Params {
    pub peer: String,
    pub idle_window_s: i64,
    pub poll_interval_s: Option<u64>,
    pub price_gpu: f64,
    pub max_duration_s: i64,
    pub dry_run: bool,
    pub once: bool,
}

fn say(message: &str) {
    println!("[{}] {message}", chrono::Utc::now().to_rfc3339());
}

/// Run the loop until it is stopped, or for one poll with `--once`.
pub(super) async fn auto_list(params: Params) -> Result<(), CmdError> {
    if params.poll_interval_s.is_none() && !params.dry_run {
        return Err(CmdError::usage(
            "lending needs --poll-interval-s: the loop polls our queue at that period, and the \
             lending worker polls the peer's queue at the same one",
        ));
    }
    if params.poll_interval_s.is_none() && !params.once {
        return Err(CmdError::usage(
            "follow needs --poll-interval-s, the seconds between queue polls, unless it runs \
             --once",
        ));
    }
    if params.idle_window_s.is_negative() || params.max_duration_s.is_negative() {
        return Err(CmdError::usage(
            "--idle-window-s and --max-duration-s are seconds and cannot be negative",
        ));
    }
    if !params.price_gpu.is_finite() || params.price_gpu.is_sign_negative() {
        return Err(CmdError::usage(format!(
            "--price-gpu {} is not a price; give US dollars per GPU-hour",
            params.price_gpu
        )));
    }
    let store = our_store().await?;
    let declared = declaration::require(&store, &params.peer)
        .await
        .map_err(cmd)?;
    let host = system_hostname();
    let max_duration_s = Some(params.max_duration_s).filter(|seconds| seconds.is_positive());
    let _held = if params.dry_run {
        say("DRY-RUN: decisions only; no worker starts and no offer is written");
        None
    } else {
        let program = std::env::current_exe().map_err(|error| {
            CmdError::declaration(format!("this program's path cannot be read: {error}"))
        })?;
        Some(crate::deploy::role_lock::take(
            &crate::config_file::home().map_err(CmdError::declaration)?,
            &loop_role(&params.peer),
            &program,
        )?)
    };
    let mut cadence = params
        .poll_interval_s
        .map(|seconds| tokio::time::interval(Duration::from_secs(seconds)));
    let mut current = start_offer(&store, &host, &params).await?;
    let mut lending: Option<Lending> = None;
    let mut drained = false;
    let mut idle_since: Option<Instant> = None;
    loop {
        if let Some(cadence) = cadence.as_mut() {
            cadence.tick().await;
        }
        if let Some(mut active) = lending.take() {
            match active.exited().map_err(cmd)? {
                Some(end) => {
                    let session = active.finish(&params.peer, end);
                    if let Some(end) = &session.end {
                        say(&format!("session ended: {end}"));
                    }
                    current.session = Some(session);
                    drained = false;
                    record(&store, &mut current, "session ended", &params).await?;
                }
                None => lending = Some(active),
            }
        }
        if !params.dry_run {
            if let Some(stored) = offer::load(&store, &host, &params.peer)
                .await
                .map_err(cmd)?
            {
                current.listed = stored.listed;
                current.price_gpu = stored.price_gpu;
            }
        }
        let state = is_stado_busy(&store, &host).await.map_err(CmdError::from)?;
        let held = match crate::inference::reservation::active() {
            Ok(Some(reservation)) => Some(format!(
                "inference reservation '{}' holds the cards",
                reservation.deployment
            )),
            Ok(None) => None,
            Err(error) => Some(format!(
                "an inference reservation that cannot be read ({error}) may hold the cards"
            )),
        };
        let over_time = match (&lending, max_duration_s) {
            (Some(active), Some(limit)) => active.seconds_running().map_err(cmd)? >= limit,
            _ => false,
        };
        let withdraw = if !current.listed {
            Some("unlisted by the operator".to_string())
        } else if let Some(held) = held {
            Some(held)
        } else if !state.idle {
            Some(format!(
                "our queue has work (queued={}, running_here={})",
                state.queued, state.running_here
            ))
        } else if over_time {
            Some("the session ran its --max-duration-s".to_string())
        } else {
            None
        };
        if let Some(reason) = withdraw {
            idle_since = None;
            match &lending {
                Some(active) if !drained => {
                    active.drain(&reason).map_err(cmd)?;
                    drained = true;
                    say(&format!("draining the lending worker: {reason}"));
                    record(
                        &store,
                        &mut current,
                        &format!("draining: {reason}"),
                        &params,
                    )
                    .await?;
                }
                Some(_) => say(&format!(
                    "waiting for the drained worker to finish: {reason}"
                )),
                None => {
                    record(
                        &store,
                        &mut current,
                        &format!("not lending: {reason}"),
                        &params,
                    )
                    .await?
                }
            }
        } else if lending.is_none() {
            let now = Instant::now();
            let idle = now.duration_since(*idle_since.get_or_insert(now)).as_secs();
            let window = params.idle_window_s.unsigned_abs();
            if idle < window {
                say(&format!("idle {idle}s/{window}s"));
            } else if params.dry_run {
                say(&format!(
                    "DRY-RUN would lend {host} to {} at ${}/GPU-hour once its queue holds work",
                    params.peer, current.price_gpu
                ));
            } else {
                lending = lend(&store, &declared, &mut current, &params, max_duration_s).await?;
            }
        }
        if params.once {
            if let Some(active) = &lending {
                active
                    .drain("follow --once ends after one poll")
                    .map_err(cmd)?;
            }
            return Ok(());
        }
    }
}

/// Start a session when the peer's queue holds work.
async fn lend(
    store: &JobStorage,
    declared: &PeerDeclaration,
    current: &mut Offer,
    params: &Params,
    max_duration_s: Option<i64>,
) -> Result<Option<Lending>, CmdError> {
    let peer_store = remote::open(declared).await.map_err(cmd)?;
    let queue = remote::queue_state(&peer_store).await.map_err(cmd)?;
    let Some(empty) = queue
        .get("queue_empty")
        .and_then(serde_json::Value::as_bool)
    else {
        return Err(CmdError::unreachable(
            "the peer queue reading carries no queue_empty",
        ));
    };
    if empty {
        record(
            store,
            current,
            "idle, and the peer's queue is empty",
            params,
        )
        .await?;
        return Ok(None);
    }
    let Some(poll) = params.poll_interval_s else {
        return Err(CmdError::usage("lending needs --poll-interval-s"));
    };
    let active = Lending::start(declared, poll, max_duration_s)
        .await
        .map_err(cmd)?;
    say(&format!(
        "LENDING to {} (worker pid {}, home {})",
        params.peer, active.session.pid, active.session.home
    ));
    current.session = Some(active.session.clone());
    record(store, current, "lending", params).await?;
    Ok(Some(active))
}
