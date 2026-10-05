//! The capacity broadcast, kept at the agent's poll period while the tick
//! works.
//!
//! "Is this agent alive" is answered by the loop going around; "may I have
//! work" is answered by a store read that is allowed to be slow. One thread of
//! control owned both, so a slow claimable-job listing also held the host's
//! broadcast back. This task republishes the tick's last snapshot on the
//! operator's poll period while the tick works.
//!
//! # Why this is not a liveness formality
//!
//! The republish is deliberately NOT unconditional, because a broadcast that
//! keeps arriving from a wedged process would route release builds to a host
//! that will never claim them. The tick stamps
//! [`CapacityHeartbeat::record_tick_start`] at the top of every iteration and
//! [`CapacityHeartbeat::record_phase`] before every call it awaits. This task
//! republishes when the loop has gone around since its last republish, or
//! while the call the tick is in has run no longer than the longest that same
//! call has ever taken to return in this process. Every publication promises
//! the next one within the poll period (`next_by`), so a host the tick keeps
//! busy in a call that is slow but no slower than it has been stays spoken for,
//! and a host whose tick sits in a call past anything it has done before, or in
//! a call it has never finished, stops being spoken for: its promise passes and
//! `host gates` refuses dispatch to it. No window of anyone's choosing decides
//! which: the tick's own measured history does.
//!
//! Nothing here computes capacity. It republishes verbatim what the tick last
//! published, so a host cannot advertise resources the tick has not measured.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::queue::capacity::{publish_capacity, CapacitySnapshot};
use crate::queue::JobStorage;

struct Shared {
    snapshot: Option<CapacitySnapshot>,
    tick_started: Instant,
    /// The phase the tick entered last and when, so a stall names where the
    /// loop is waiting instead of only how long ago it last went around.
    phase: (&'static str, Instant),
    /// The longest each phase has taken to return in this process: the
    /// envelope inside which a tick still in that phase is slow, not stuck.
    longest: HashMap<&'static str, Duration>,
}

impl Shared {
    /// Close the phase the tick was in: it has returned, so its duration is
    /// a measured fact about how long that call can take.
    fn close_phase(&mut self, now: Instant) {
        let (phase, entered) = self.phase;
        let took = now.saturating_duration_since(entered);
        let longest = self.longest.entry(phase).or_default();
        if took > *longest {
            *longest = took;
        }
    }

    /// Whether the phase the tick is in now has run no longer than that
    /// phase has ever taken to return. A phase never seen returning has no
    /// envelope, and a tick in it is not vouched for.
    fn within_measured(&self, now: Instant) -> bool {
        let (phase, entered) = self.phase;
        self.longest
            .get(phase)
            .is_some_and(|longest| now.saturating_duration_since(entered) <= *longest)
    }
}

/// Handle the tick holds: one stamp per iteration, one snapshot per publish.
#[derive(Clone)]
pub struct CapacityHeartbeat {
    shared: Arc<Mutex<Shared>>,
}

/// A running heartbeat. Dropping it stops the republish, so an agent that
/// exits for a release handoff does not leave a task speaking for it.
pub struct HeartbeatTask {
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for HeartbeatTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl Default for CapacityHeartbeat {
    fn default() -> Self {
        Self::new()
    }
}

impl CapacityHeartbeat {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                snapshot: None,
                tick_started: Instant::now(),
                phase: ("start", Instant::now()),
                longest: HashMap::new(),
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The tick is going around. Called at the top of every iteration; the
    /// call the last iteration ended in has returned.
    pub fn record_tick_start(&self) {
        let now = Instant::now();
        let mut shared = self.lock();
        shared.close_phase(now);
        shared.tick_started = now;
        shared.phase = ("iteration", now);
    }

    /// The tick is entering `phase` (the call it is about to await); the
    /// one it was in has returned.
    pub fn record_phase(&self, phase: &'static str) {
        let now = Instant::now();
        let mut shared = self.lock();
        shared.close_phase(now);
        shared.phase = (phase, now);
    }

    /// The tick published this. Republished verbatim until it publishes again.
    pub fn record_published(&self, snapshot: CapacitySnapshot) {
        self.lock().snapshot = Some(snapshot);
    }

    /// Start republishing every `poll`, the period every publication of this
    /// process promises its next one within. `log_fn` is the agent's own
    /// logger, so a refused republish is as visible as a refused tick publish.
    pub fn spawn(
        &self,
        store: JobStorage,
        consumer_id: String,
        kind: String,
        poll: Duration,
        log_fn: fn(&str),
    ) -> HeartbeatTask {
        crate::queue::capacity::declare_publisher_cadence(poll);
        let shared = self.shared.clone();
        let handle = tokio::spawn(async move {
            let mut spoken_for: Option<Instant> = None;
            loop {
                tokio::time::sleep(poll).await;
                let now = Instant::now();
                let (snapshot, tick_started, (phase, phase_started), within, longest) = {
                    let shared = shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    (
                        shared.snapshot.clone(),
                        shared.tick_started,
                        shared.phase,
                        shared.within_measured(now),
                        shared.longest.get(shared.phase.0).copied(),
                    )
                };
                let Some(snapshot) = snapshot else {
                    // Nothing measured yet. The tick's own first publish is
                    // the first thing this host says.
                    continue;
                };
                if spoken_for == Some(tick_started) && !within {
                    log_fn(&format!(
                        "heartbeat: the tick has been in {phase} for {}s ({}; its iteration \
                         started {}s ago); this host is not spoken for until the loop moves \
                         again",
                        phase_started.elapsed().as_secs(),
                        match longest {
                            Some(longest) => format!(
                                "the longest it has taken to return in this process is {}s",
                                longest.as_secs()
                            ),
                            None => "it has not returned once in this process".to_string(),
                        },
                        tick_started.elapsed().as_secs()
                    ));
                    continue;
                }
                spoken_for = Some(tick_started);
                match publish_capacity(&store, &consumer_id, &kind, &snapshot).await {
                    Ok(()) => log_fn(&format!(
                        "heartbeat: republished accepting_jobs={} running_jobs={} \
                         available_cpu_cores={} free_vram_gb={} while the tick works",
                        snapshot.accepting_jobs,
                        snapshot.running_jobs,
                        snapshot.available_cpu_cores,
                        snapshot.free_vram_gb
                    )),
                    Err(exc) => log_fn(&format!(
                        "heartbeat: capacity republish REFUSED by the store: {exc}"
                    )),
                }
            }
        });
        HeartbeatTask { handle }
    }
}
