//! A worker whose store a placement move fences keeps running and works
//! again once the fence is lifted, instead of ending the host's process.
//!
//! One isolated deployment: `config init` seeds a local store and a CPU job is
//! submitted. `stado storage fence arm` then fences the store as a role move
//! does before its last copy, and the real `stado serve` worker starts. It
//! must say twice that the store refused it with the fence's own sentence,
//! which names the transaction, so the process outlived its first refused
//! tick. `stado storage fence disarm` lifts the fence, and the same worker
//! must then start the job and see it complete.
use crate::deployment::{line_with, line_with_any, Deployment};
use serde_json::json;

const TRANSACTION: &str = "fenced-store-journey";

#[test]
fn a_fenced_store_holds_the_worker_off_without_ending_it() {
    let mut deployment = Deployment::start("fenced_store");
    deployment.cli(&["submit", "--run-id", "fenced-store-journey", "true"]);
    deployment.cli(&[
        "storage",
        "fence",
        "arm",
        "--transaction",
        TRANSACTION,
        "--reason",
        "journey",
        "--json",
    ]);
    let fenced =
        format!("local storage writes are fenced by stado storage fence transaction {TRANSACTION}");
    let lines = deployment.serve(&["--worker", "--kind", "local", "--poll-seconds", "1"]);
    let first = line_with_any(&lines, &[&fenced, "ordinary work remains blocked"]);
    assert!(
        first.contains(&fenced),
        "the worker on this host refuses ordinary work, so the journey cannot run here: {first}"
    );
    let again = line_with(&lines, &fenced);
    deployment.report["fenced"] = json!([first, again]);

    deployment.cli(&[
        "storage",
        "fence",
        "disarm",
        "--transaction",
        TRANSACTION,
        "--reason",
        "journey",
        "--json",
    ]);
    let started = line_with(&lines, "Started job");
    deployment.report["started"] = json!(started);
    for _ in lines.iter() {
        if !deployment.documents("completed").is_empty() {
            break;
        }
    }
    assert_eq!(
        deployment.documents("completed").len(),
        std::num::NonZeroUsize::MIN.get(),
        "the job submitted before the fence completes once it is lifted"
    );
    deployment.pass();
}
