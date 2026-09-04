//! The capacity broadcast, kept at its declared cadence while the tick works.
//!
//! # Why this exists
//!
//! [`crate::constants::CAPACITY_STALE_SECONDS`] (180 s) is the window the fleet
//! judges a host's liveness by, and
//! [`crate::constants::CAPACITY_HEARTBEAT_INTERVAL_S`] is the design's own
//! answer to it: publish at a third of the window. The agent tick published
//! once per iteration, so the cadence was really "however long an iteration
//! takes", and on a saturated object store an iteration takes as long as the
//! store does. Bounding the tick's reads fixed the pathological case (17 and
//! 43 minute iterations, measured on 2026-09-03) but produced the next one:
//! with the whole tick inside a 90 s budget, the claimable-job listing — the
//! heaviest read and the only one claiming depends on — lapsed on every single
//! tick against a store answering in tens of seconds:
//!
//! ```text
//! 22:49:58 loop: claimable-job read exhausted this tick's 90s store budget
//! 22:51:38 loop: claimable-job read exhausted this tick's 90s store budget
//! ```
//!
//! A host that stays fresh by never claiming is not fixed. Freshness and
//! claiming were competing for one budget because one thread of control owned
//! both, and they are not the same question: "is this agent alive" is answered
//! by the loop going around, "may I have work" is answered by a store read
//! that is allowed to be slow.
//!
//! # Why this is not a liveness formality
//!
//! The republish is deliberately NOT unconditional, because a broadcast that
//! keeps arriving from a wedged process is exactly the control-turned-formality
//! the fleet cannot afford: it would route release builds to a host that will
//! never claim them.
//!
//! So the tick stamps [`CapacityHeartbeat::record_tick_start`] at the top of
//! every iteration, and this task republishes the last snapshot ONLY while
//! that stamp is younger than [`crate::constants::AGENT_TICK_PROGRESS_TTL_S`].
//! A loop that is slow keeps its host selectable; a loop that has stopped
//! going around stops being spoken for, its row ages past the window, and
//! `host gates` refuses dispatch to it exactly as before. The signal still
//! means "this agent is going around its loop" — it just no longer means "and
//! it finished an iteration in the last three minutes", which was never the
//! question.
//!
//! Nothing here computes capacity. It republishes verbatim what the tick last
//! published, so a host cannot advertise resources the tick has not measured.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::constants;
use crate::queue::capacity::{publish_capacity, CapacitySnapshot};
use crate::queue::JobStorage;
struct Shared {
    snapshot: Option<CapacitySnapshot>,
    tick_started: Instant,
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

    /// The tick published this. Republished verbatim until it publishes again.
    pub fn record_published(&self, snapshot: CapacitySnapshot) {
        self.lock().snapshot = Some(snapshot);
    }

    /// Start republishing. `log_fn` is the agent's own logger, so a refused
    /// republish is as visible as a refused tick publish.
    pub fn spawn(
        &self,
        store: JobStorage,
        consumer_id: String,
        kind: String,
        log_fn: fn(&str),
    ) -> HeartbeatTask {
        let shared = self.shared.clone();
        let handle = tokio::spawn(async move {
            let interval = Duration::from_secs(constants::CAPACITY_HEARTBEAT_INTERVAL_S);
            let progress_ttl = Duration::from_secs(constants::AGENT_TICK_PROGRESS_TTL_S);
            let mut announced_stall = false;
            loop {
                tokio::time::sleep(interval).await;
                let (snapshot, tick_age) = {
                    let shared = shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    (shared.snapshot.clone(), shared.tick_started.elapsed())
                };
                let Some(snapshot) = snapshot else {
                    // Nothing measured yet. The tick's own first publish is
                    // the first thing this host says.
                    continue;
                };
                if tick_age > progress_ttl {
                    if !announced_stall {
                        announced_stall = true;
                        log_fn(&format!(
                            "heartbeat: the tick has not started an iteration for {}s, past the \
                             {}s progress window; this host will NOT be spoken for until the loop \
                             moves again and its capacity row is allowed to go stale",
                            tick_age.as_secs(),
                            constants::AGENT_TICK_PROGRESS_TTL_S
                        ));
                    }
                    continue;
                }
                announced_stall = false;
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
