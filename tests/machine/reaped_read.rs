//! `stado machine status <job>` for a job the run reaper has retired, on a
//! real local queue holding several reaped runs: the read opens the one run
//! manifest the index names and no other — counted from the `czekam` lines
//! the built `stado` writes for every manifest lock it takes — and a job
//! retained before the index existed (its index entry removed, as a store
//! from before the index has none) is still found through every run.
#[path = "../status/fixture.rs"]
mod fixture;
use fixture::{manifest_reads, Store};
use serde_json::{json, Value};

/// The runs this case submits, one job each; the one in the middle is read.
const RUNS: &[&str] = &["machine-read-a", "machine-read-b", "machine-read-c"];
const READ: &str = "machine-read-b";

/// `stado machine status` of one job: the envelope it printed and the run
/// manifests it read on the way.
fn status(store: &mut Store, job_id: &str) -> (Value, Vec<String>) {
    let output = store.run(&["machine", "status", job_id]);
    assert!(
        output.status.success(),
        "machine status {job_id}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value =
        serde_json::from_slice(&output.stdout).expect("the machine interface prints one JSON line");
    assert_eq!(envelope["ok"], true, "{envelope}");
    (envelope, manifest_reads(&String::from_utf8_lossy(&output.stderr)))
}

#[test]
fn a_reaped_job_is_read_from_the_one_manifest_its_index_names() {
    let mut store = Store::new("machine-reaped-read");
    let jobs: Vec<(&str, String)> = RUNS
        .iter()
        .map(|run| (*run, store.submit(run, "echo machine")))
        .collect();
    for (_, job) in &jobs {
        store.cancel(job);
    }
    store.reap(jobs.len(), "1");
    let (run, job_id) = jobs
        .iter()
        .find(|(run, _)| *run == READ)
        .expect("the run to read was submitted");
    let manifest = format!("{run}.json");
    let index = store.store().join(format!("runs/jobs/{job_id}"));
    assert_eq!(
        std::fs::read_to_string(&index).expect("the reaper indexes the job by its run").trim(),
        *run,
        "the index names the run that retained the job"
    );

    // Indexed: the one manifest the index names, and nothing else.
    let (envelope, reads) = status(&mut store, job_id);
    assert_eq!(envelope["result"]["job"]["job_id"], *job_id, "{envelope}");
    assert_eq!(envelope["result"]["job"]["state"], "cancelled", "{envelope}");
    assert_eq!(reads, vec![manifest.clone()], "{envelope}");
    store.observe("indexed", json!({"manifest_reads": reads, "job": envelope["result"]["job"]}));

    // Retained before the index existed: still found, through every run up
    // to its own, and that read writes the index back, so the next read
    // opens one manifest again.
    std::fs::remove_file(&index).expect("the index entry is removed");
    let (envelope, reads) = status(&mut store, job_id);
    assert_eq!(envelope["result"]["job"]["job_id"], *job_id, "{envelope}");
    assert_eq!(envelope["result"]["job"]["state"], "cancelled", "{envelope}");
    assert!(reads.contains(&manifest), "{reads:?}");
    let one = vec![manifest.clone()];
    assert!(reads.len() > one.len(), "an unindexed job reads other runs: {reads:?}");
    store.observe("unindexed", json!({"manifest_reads": reads}));
    assert_eq!(
        std::fs::read_to_string(&index).expect("the read indexed the job by its run").trim(),
        *run,
        "the walk writes the index entry it had to do without"
    );
    let (_, reads) = status(&mut store, job_id);
    assert_eq!(reads, one, "the re-indexed job opens one manifest again");
    store.observe("reindexed", json!({"manifest_reads": reads}));
    store.pass();
}
