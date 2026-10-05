//! A measurement the queue records is used by the very next coordinator
//! tick: the sizing map is rebuilt when the completed and failed records
//! change, never after a window of its own.
//!
//! One isolated deployment: `config init` seeds a local store, a job naming
//! a model nothing has measured yet is submitted, and the real
//! `stado serve --control-plane local` ticks over it. Once a tick has passed,
//! a completed record measuring that model appears in the store; by the end
//! of the next tick that starts after it, the queued job must carry the
//! measured peak.
#[path = "deployment.rs"]
mod deployment;

use deployment::{line_with, Deployment};
use serde_json::json;
use std::fs;

const MODEL: &str = "sizing-journey/measured-model";
const MEASURED_PEAK_GB: i64 = 11;
const TICK: &str = "tick scheduled=";

#[test]
fn the_next_tick_sizes_a_queued_job_from_a_new_measurement() {
    let mut deployment = Deployment::start("sizing");
    let command = format!("true --model {MODEL}");
    deployment.cli(&["submit", "--run-id", "sizing-journey", &command]);
    let lines = deployment.serve(&[
        "--control-plane",
        "local",
        "--control-plane-interval-seconds",
        "5",
    ]);

    // A tick over a queue whose model nothing has measured.
    deployment.report["tick_before_measurement"] = json!(line_with(&lines, TICK));
    let job = deployment
        .documents("queue")
        .into_iter()
        .next()
        .expect("the submitted job stays queued: nothing claims it here");
    assert_eq!(job["gpu_mem_gb"], json!(0), "unmeasured model: {job}");

    // The measurement a finished run of that model records.
    let completed = deployment.store().join("completed");
    fs::create_dir_all(&completed).unwrap();
    let record = json!({
        "job_id": "sizing-journey-measured",
        "state": "completed",
        "command": command,
        "peak_vram_gb": MEASURED_PEAK_GB,
        "peak_vram_per_gpu": true,
    });
    fs::write(
        completed.join("sizing-journey-measured.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    deployment.report["measurement"] = record;

    // The tick in flight when the record landed may have listed before it;
    // the one after it cannot have.
    deployment.report["ticks_after_measurement"] =
        json!([line_with(&lines, TICK), line_with(&lines, TICK)]);
    let job = deployment
        .documents("queue")
        .into_iter()
        .next()
        .expect("the job is still queued");
    deployment.report["queued_job"] = job.clone();
    assert_eq!(
        job["gpu_mem_gb"],
        json!(MEASURED_PEAK_GB),
        "the tick after the measurement must size the job from it: {job}"
    );
    deployment.pass();
}
