//! The disk-cleanup pass, lifted off the agent tick's critical path.
//!
//! # The defect this exists to make impossible
//!
//! A capacity publication is what makes a host selectable as a release
//! builder: `cli::release_submit::builder` reads live consumer capacity and
//! refuses outright when nothing fresh names the platform, and
//! `queue::capacity::read_consumer_capacity_at` drops any publication older
//! than [`crate::constants::CAPACITY_STALE_SECONDS`] (180s). The design's own
//! answer to that cutoff is
//! [`crate::constants::CAPACITY_HEARTBEAT_INTERVAL_S`] — "always fresh before
//! the stale threshold" — one third of it.
//!
//! The agent tick used to `await run_cleanup_once` BEFORE it reached its
//! capacity publication, on the same task, with no concurrency. Every second
//! the janitor spent was a second the publication was not written. Measured on
//! charless-mac-mini on 2026-09-03: `duration_ms: 818021` — 13.6 minutes — for
//! a pass whose own verdict was `healthy_noop` on a host with 19.8 GB free,
//! against a policy `check_interval_seconds` of 300, so passes ran effectively
//! back to back. The builder was therefore selectable for roughly three
//! minutes in every fourteen, and two weles-worker releases were refused that
//! day with `no live fleet builder is broadcasting verified release_platform
//! darwin-arm64 ... listed 0 live consumer(s)` against a builder that was
//! healthy, running and correctly declared. A release on this fleet succeeded
//! or failed by luck.
//!
//! # The mechanism, and why this one
//!
//! The pass runs on its own task at its own cadence; the tick reads only
//! reports from passes that have already COMPLETED, and never waits for one in
//! progress. Publication then happens at the heartbeat interval no matter how
//! long a pass takes, which is the invariant that was being violated by an
//! order of magnitude.
//!
//! The alternative the evidence report also offered — publish first, then run
//! the pass in the same tick — was rejected: it fixes the ORDER but not the
//! CADENCE. A tick whose body still takes 818 seconds publishes every 818
//! seconds wherever the publish sits inside it. Only getting the pass off the
//! critical path restores "published at least every 60s".
//!
//! Nothing here changes cleanup itself. `run_cleanup_once` keeps its own
//! cross-process lock, its own policy resolution and its own caps, and it is
//! still invoked at the tick's poll cadence — just not from the tick. The only
//! observable change is that `diag.disk_cleanup` describes the most recently
//! completed pass rather than one that finished microseconds ago. Every
//! consumer of that field already tolerates that, and `host gates` reports a
//! publication's age rather than assuming freshness.
//!
//! (Recorded, not chased, because it belongs to whoever owns the pass: a
//! `healthy_noop` pass costing 818 seconds to free nothing on a host with
//! 19.8 GB free is doing work nobody reads.)

use std::future::Future;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

/// The janitor's latest completed report, readable by the tick without waiting.
///
/// Cloneable: the tick holds one handle and the janitor task holds another.
#[derive(Clone, Default)]
pub struct JanitorReports {
    latest: Arc<Mutex<Option<Value>>>,
    /// How many passes have completed. The tick logs the first one so an
    /// operator can tell "no pass has finished yet" from "the janitor is
    /// wedged".
    completed: Arc<AtomicI64>,
    /// The active slot count the next pass should be told about, written by the
    /// tick and read by the janitor. Shared as a number rather than passed in,
    /// because the pass outlives the tick that started it.
    active_slots: Arc<AtomicI64>,
}

impl JanitorReports {
    pub fn new() -> Self {
        Self::default()
    }

    /// The most recently COMPLETED cleanup report, or `None` when no pass has
    /// finished yet.
    ///
    /// This never awaits, and MUST never be made to: waiting here is exactly
    /// the defect. A tick that blocks on an in-flight pass stops publishing
    /// capacity, and a host that stops publishing capacity stops being a
    /// release builder anywhere in the fleet.
    pub fn latest(&self) -> Option<Value> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// How many passes have completed since this agent started.
    pub fn completed_passes(&self) -> i64 {
        self.completed.load(Ordering::Relaxed)
    }

    /// Tell the janitor how many slots are active, for the next pass it starts.
    pub fn set_active_slots(&self, count: i64) {
        self.active_slots.store(count, Ordering::Relaxed);
    }

    /// What the next pass should be told. Public for the janitor body and for
    /// tests that drive one.
    pub fn active_slots(&self) -> i64 {
        self.active_slots.load(Ordering::Relaxed)
    }

    fn record(&self, report: Value) {
        *self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(report);
        self.completed.fetch_add(1, Ordering::Relaxed);
    }

    /// Run `pass` forever on a thread of its own, recording each completed
    /// report, and waiting `interval` between passes.
    ///
    /// A thread with its own current-thread runtime rather than
    /// `tokio::spawn`, for a concrete reason: `disk_cleanup::run_cleanup_once`
    /// takes its logger as `&mut dyn FnMut(&str)`, which is not `Send`, so any
    /// future holding one cannot be spawned onto the agent's multi-threaded
    /// runtime. The alternatives were to widen the cleanup engine's own
    /// signature — the pass is explicitly not ours to change here — or to keep
    /// the pass on the tick's task and poll it by hand, which reintroduces the
    /// coupling this module exists to remove. Its own thread costs one thread
    /// and decouples the two completely.
    ///
    /// The pass is handed the active slot count at the moment it starts. A pass
    /// that runs long delays only the next pass, never a publication.
    pub fn spawn_janitor<F, Fut>(&self, interval: Duration, mut pass: F) -> JanitorTask
    where
        F: FnMut(i64) -> Fut + Send + 'static,
        Fut: Future<Output = Value>,
    {
        let reports = self.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_signal = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("stado-agent-janitor".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    // Without a runtime there is no janitor, and that must not
                    // take the agent's capacity broadcast down with it: the
                    // whole point of this module is that publication survives
                    // whatever the janitor does.
                    Err(error) => {
                        crate::providers::local::agent::agent_log(&format!(
                            "janitor: no runtime, disk cleanup will not run this process: {error}"
                        ));
                        return;
                    }
                };
                runtime.block_on(async move {
                    while !stop_signal.load(Ordering::Relaxed) {
                        let report = pass(reports.active_slots()).await;
                        reports.record(report);
                        tokio::time::sleep(interval).await;
                    }
                });
            })
            .ok();
        JanitorTask { stop, handle }
    }
}

/// A running janitor. Dropping it stops the pass loop after the pass in flight,
/// so an agent that exits for a release handoff does not leave one behind.
pub struct JanitorTask {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl JanitorTask {
    /// Ask the janitor to stop after the pass in flight. Exposed so a test can
    /// end its janitor deterministically instead of relying on drop order.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for JanitorTask {
    fn drop(&mut self) {
        self.stop();
        // Deliberately NOT joined: a pass can legitimately be minutes long --
        // 818 seconds was measured -- and blocking an agent's shutdown on the
        // janitor is the same mistake as blocking its publication on one.
        drop(self.handle.take());
    }
}
