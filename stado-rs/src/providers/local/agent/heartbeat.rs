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
//! [`CapacityHeartbeat::record_tick_start`] at the top of every iteration, and
//! this task republishes only when that stamp has moved since its own last
//! republish. A loop that has stopped going around stops being spoken for,
//! its row ages, and `host gates` refuses dispatch to it.
//!
//! Nothing here computes capacity. It republishes verbatim what the tick last
//! published, so a host cannot advertise resources the tick has not measured.

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
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The tick is going around. Called at the top of every iteration.
    pub fn record_tick_start(&self) {
        self.lock().tick_started = Instant::now();
    }

    /// The tick is entering `phase` (the call it is about to await).
    pub fn record_phase(&self, phase: &'static str) {
        self.lock().phase = (phase, Instant::now());
    }

    /// The tick published this. Republished verbatim until it publishes again.
    pub fn record_published(&self, snapshot: CapacitySnapshot) {
        self.lock().snapshot = Some(snapshot);
    }

    /// Start republishing every `poll`. `log_fn` is the agent's own logger, so
    /// a refused republish is as visible as a refused tick publish.
    pub fn spawn(
        &self,
        store: JobStorage,
        consumer_id: String,
        kind: String,
        poll: Duration,
        log_fn: fn(&str),
    ) -> HeartbeatTask {
        let shared = self.shared.clone();
        let handle = tokio::spawn(async move {
            let mut spoken_for: Option<Instant> = None;
            loop {
                tokio::time::sleep(poll).await;
                let (snapshot, tick_started, (phase, phase_started)) = {
                    let shared = shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    (shared.snapshot.clone(), shared.tick_started, shared.phase)
                };
                let Some(snapshot) = snapshot else {
                    // Nothing measured yet. The tick's own first publish is
                    // the first thing this host says.
                    continue;
                };
                if spoken_for == Some(tick_started) {
                    log_fn(&format!(
                        "heartbeat: the tick has been in {phase} for {}s (its iteration \
                         started {}s ago); this host is not spoken for until the loop moves \
                         again",
                        phase_started.elapsed().as_secs(),
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
