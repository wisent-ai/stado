//! The janitor pass, lifted off the agent tick's critical path.
//!
//! # The defect this exists to make impossible
//!
//! A capacity publication is what makes a host selectable as a release
//! builder: `cli::release_submit::builder` reads live consumer capacity and
//! refuses outright when nothing fresh names the platform, and
//! `queue::capacity::read_consumer_capacity_at` drops any publication past
//! the next one its publisher promised (`next_by`).
//!
//! Awaiting `run_cleanup_once` BEFORE the capacity publication, on the same
//! task, makes every second the janitor spends a second the publication is
//! not written. A pass that walks a full disk can take minutes, and a builder
//! that stops publishing for that long is refused as a release builder with
//! `no live fleet builder is broadcasting verified release_platform
//! darwin-arm64 ... listed 0 live consumer(s)` while it is healthy, running
//! and correctly declared.
//!
//! # The mechanism, and why this one
//!
//! The pass runs on its own task at its own cadence; the tick reads only
//! reports from passes that have already COMPLETED, and never waits for one in
//! progress. Publication then happens on the agent's poll period no matter
//! how long a pass takes.
//!
//! The alternative the evidence report also offered — publish first, then run
//! the pass in the same tick — was rejected: it fixes the ORDER but not the
//! CADENCE. A tick whose body still takes 818 seconds publishes every 818
//! seconds wherever the publish sits inside it. Only getting the pass off the
//! critical path restores "published at least every 60s".
//!
//! Nothing here changes cleanup itself. `run_cleanup_once` keeps its own
//! cross-process lock and the disk-full rule, and it is still invoked at the
//! tick's poll cadence — just not from the tick. The only observable change is
//! that `diag.space_reclaim` describes the most recently completed pass rather
//! than one that finished microseconds ago, and `host gates` reports a
//! publication's age rather than assuming freshness.

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
    /// The active job count the next pass should be told about, written by the
    /// tick and read by the janitor. Shared as a number rather than passed in,
    /// because the pass outlives the tick that started it.
    active_jobs: Arc<AtomicI64>,
}

impl JanitorReports {
    pub fn new() -> Self {
        Self::default()
    }

    /// What the tick publishes as `space_reclaim`: the most recently COMPLETED
    /// cleanup report, `{"outcome": "no_pass_completed_yet"}` before the first
    /// one, so a missing pass never reads like a wedged janitor, or
    /// `{"outcome": "unreadable"}` with the cause when a panic in a pass
    /// poisoned the report.
    ///
    /// This never awaits, and MUST never be made to: waiting here is exactly
    /// the defect. A tick that blocks on an in-flight pass stops publishing
    /// capacity, and a host that stops publishing capacity stops being a
    /// release builder anywhere in the fleet.
    pub fn latest(&self) -> Value {
        match self.latest.lock() {
            Ok(latest) => match latest.as_ref() {
                Some(report) => report.clone(),
                None => serde_json::json!({"outcome": "no_pass_completed_yet"}),
            },
            Err(poisoned) => serde_json::json!({
                "outcome": "unreadable",
                "error": format!("a janitor pass panicked while recording its report: {poisoned}"),
            }),
        }
    }

    /// Tell the janitor how many jobs are active for the next pass it starts.
    pub fn set_active_jobs(&self, count: i64) {
        self.active_jobs.store(count, Ordering::Relaxed);
    }

    /// What the next pass should be told. Public for the janitor body and for
    /// tests that drive one.
    pub fn active_jobs(&self) -> i64 {
        self.active_jobs.load(Ordering::Relaxed)
    }

    fn record(&self, report: Value) {
        match self.latest.lock() {
            Ok(mut latest) => *latest = Some(report),
            // The tick reads the poisoned lock as `unreadable` with its cause;
            // this pass's report is said in the agent log so it is not lost.
            Err(poisoned) => crate::providers::local::agent::agent_log(&format!(
                "janitor: a pass report was not recorded, the report lock is poisoned \
                 ({poisoned}); report: {report}"
            )),
        }
    }

    /// Run `pass` forever on a thread of its own, recording each completed
    /// report: one pass at once, then one every `interval`.
    ///
    /// A thread with its own current-thread runtime rather than
    /// `tokio::spawn`, for a concrete reason: `space_reclaim::run_cleanup_once`
    /// takes its logger as `&mut dyn FnMut(&str)`, which is not `Send`, so any
    /// future holding one cannot be spawned onto the agent's multi-threaded
    /// runtime. The alternatives were to widen the cleanup engine's own
    /// signature — the pass is explicitly not ours to change here — or to keep
    /// the pass on the tick's task and poll it by hand, which reintroduces the
    /// coupling this module exists to remove. Its own thread costs one thread
    /// and decouples the two completely.
    ///
    /// The pass is handed the active job count at the moment it starts. A pass
    /// that runs long delays only the next pass, never a publication.
    pub fn spawn_janitor<F, Fut>(&self, interval: Duration, mut pass: F) -> JanitorTask
    where
        F: FnMut(i64) -> Fut + Send + 'static,
        Fut: Future<Output = Value>,
    {
        let reports = self.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_signal = Arc::clone(&stop);
        let spawned = std::thread::Builder::new()
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
                    // A pass that runs past its slot delays only the next one.
                    let mut schedule = tokio::time::interval(interval);
                    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    while !stop_signal.load(Ordering::Relaxed) {
                        schedule.tick().await;
                        let report = pass(reports.active_jobs()).await;
                        reports.record(report);
                    }
                });
            });
        // A thread the system refuses is no janitor; the agent goes on
        // publishing and says why cleanup will not run.
        let handle = match spawned {
            Ok(handle) => Some(handle),
            Err(error) => {
                crate::providers::local::agent::agent_log(&format!(
                    "janitor: its thread was not started, disk cleanup will not run this \
                     process: {error}"
                ));
                None
            }
        };
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
