//! One phase of a tick: what a phase that fails because the fleet store did
//! not answer does to the tick, and how a phase is named in the heartbeat.

/// The called path of a phase expression: `claim::queue::claimable` from
/// `claim::queue::claimable(&store, …).await`; an expression that calls
/// nothing is named by itself.
pub(in crate::providers::local::agent::tick) fn phase_name(
    expression: &'static str,
) -> &'static str {
    match expression.split_once('(') {
        Some((called, _arguments)) => called.trim(),
        None => expression.trim(),
    }
}

/// Unwrap one phase of a tick. A fleet store that does not answer ends that
/// tick, says which store failure it was, and the next poll runs again: the
/// worker is one role of the host's one Stado process, and a gateway error
/// from the vault host's object API ending it took the resolver and every
/// service forward of the host down with it until launchd restarted the
/// process. Any other error still ends the agent visibly.
/// Before it awaits, the phase is recorded in the heartbeat under the call's
/// own path (`claim::queue::claimable`), so a tick that stops moving is named
/// by where it waits, not only by how long ago it last went around.
macro_rules! tick_phase {
    ($heartbeat:expr, $phase:expr, $log:expr) => {{
        $heartbeat.record_phase($crate::providers::local::agent::tick::phase::phase_name(
            stringify!($phase),
        ));
        tick_phase!($phase, $log)
    }};
    ($phase:expr, $log:expr) => {
        match $phase {
            Ok(value) => value,
            Err(error) => {
                let error: anyhow::Error = error.into();
                if $crate::providers::local::agent::tick::store_unavailable(&error) {
                    $log(&format!(
                        "tick did not run: the fleet store did not answer ({error:#}); the next \
                         poll runs again"
                    ));
                    continue;
                }
                return Err(error);
            }
        }
    };
}

/// [`tick_phase!`] for a phase that answers a `Step`: its value when the tick
/// goes on, the next poll when the tick is done, and the agent's end when it
/// stops.
macro_rules! tick_step {
    ($heartbeat:expr, $phase:expr, $log:expr) => {
        match tick_phase!($heartbeat, $phase, $log) {
            Step::Go(value) => value,
            Step::Done => continue,
            Step::Stop => return Ok(()),
        }
    };
}
