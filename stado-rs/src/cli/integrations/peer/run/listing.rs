//! This host's offer record as the lending loop keeps it: listed when the
//! loop starts, a session an earlier loop left running asked to drain, and
//! each change of what the loop observed written.

use crate::cli::CmdError;
use crate::providers::peer::lending::drain_left_behind;
use crate::providers::peer::{offer, Offer};
use crate::queue::JobStorage;

use super::super::cmd;
use super::{say, Params};

/// This host's offer as the loop starts: listed at the loop's price, and a
/// session a previous loop left running asked to drain.
pub(super) async fn start_offer(
    store: &JobStorage,
    host: &str,
    params: &Params,
) -> Result<Offer, CmdError> {
    let existing = offer::load(store, host, &params.peer).await.map_err(cmd)?;
    let mut current = match existing {
        Some(existing) => existing,
        None => Offer {
            peer: params.peer.clone(),
            host: host.to_string(),
            listed: true,
            price_gpu: params.price_gpu,
            changed_at: String::new(),
            changed_by: String::new(),
            state: String::new(),
            session: None,
        },
    };
    if params.dry_run {
        return Ok(current);
    }
    if let Some(session) = current
        .session
        .as_mut()
        .filter(|session| session.ended_at.is_none())
    {
        let reason = "the loop that started it ended; a new loop took the peer";
        let end = match drain_left_behind(session, reason) {
            Ok(()) => format!(
                "{reason}; its worker was asked to drain in {}",
                session.home
            ),
            Err(error) => format!("{reason}; asking its worker to drain failed: {error}"),
        };
        session.ended_at = Some(chrono::Utc::now().to_rfc3339());
        session.end = Some(end);
    }
    current.listed = true;
    current.price_gpu = params.price_gpu;
    let started = format!("started by the auto-list loop pid {}", std::process::id());
    record(store, &mut current, &started, params).await?;
    Ok(current)
}

/// Write the offer when what the loop observed changed. A dry run writes
/// nothing.
pub(super) async fn record(
    store: &JobStorage,
    current: &mut Offer,
    state: &str,
    params: &Params,
) -> Result<(), CmdError> {
    if params.dry_run {
        say(state);
        return Ok(());
    }
    if current.state == state && current.changed_by == "auto-list" {
        return Ok(());
    }
    say(state);
    current.state = state.to_string();
    current.changed_at = chrono::Utc::now().to_rfc3339();
    current.changed_by = "auto-list".to_string();
    *current = offer::save(store, current).await.map_err(cmd)?;
    Ok(())
}
