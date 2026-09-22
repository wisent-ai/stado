//! Reading one release run by identifier prefix or version, and what a
//! finished build reports it cost.

use serde_json::{json, Value};

use crate::fixture::{stderr, stdout, untouched, Store};

#[test]
fn one_release_run_is_read_by_id_prefix_or_version() {
    let store = Store::new();
    store.seed_run(
        "aaaa1111aaaa1111aaaa1111aaaa1111",
        "lake",
        "0.2.2",
        "failed",
    );
    store.seed_run(
        "bbbb2222bbbb2222bbbb2222bbbb2222",
        "lake",
        "0.2.3",
        "completed",
    );
    store.seed_run(
        "cccc3333cccc3333cccc3333cccc3333",
        "other",
        "1.0.0",
        "completed",
    );

    let out = store.stado(&["release", "status", "--run", "aaaa1111"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("run aaaa1111 lake 0.2.2 candidate failed"),
        "{text}"
    );
    assert!(
        text.contains("failure: required delivery w2 failed"),
        "{text}"
    );
    assert!(!text.contains("bbbb2222"), "only the named run: {text}");
    assert!(
        !text.contains("target="),
        "no target rows for a run question: {text}"
    );

    let out = store.stado(&["release", "status", "lake", "--version", "0.2.3", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let runs: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let ids: Vec<&str> = runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["run_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["bbbb2222bbbb2222bbbb2222bbbb2222"]);
    untouched(store.home.path());
}

#[test]
fn an_unknown_run_is_refused_naming_the_newest_runs() {
    let store = Store::new();
    store.seed_run(
        "bbbb2222bbbb2222bbbb2222bbbb2222",
        "lake",
        "0.2.3",
        "completed",
    );
    let out = store.stado(&["release", "status", "--run", "ffff"]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(
        text.contains("no release run matches run=ffff version=* product=* among the newest 120 runs; the newest are:"),
        "{text}"
    );
    assert!(
        text.contains("bbbb2222bbbb2222bbbb2222bbbb2222 lake 0.2.3"),
        "{text}"
    );
}

#[test]
fn a_finished_build_reports_what_it_cost() {
    let store = Store::new();
    let run_id = "dddd4444dddd4444dddd4444dddd4444";
    store.seed_run(run_id, "lake", "0.2.4", "completed");
    store.seed_completed_job(
        &format!("job-{run_id}"),
        "2026-09-19T21:04:30+00:00",
        "2026-09-19T21:23:04+00:00",
    );

    let out = store.stado(&["release", "status", "--run", "dddd4444"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("linux-amd64 published job=job-dddd [completed] took 18m34s"),
        "the platform line carries the job's own clock: {text}"
    );

    let out = store.stado(&["release", "status", "--run", "dddd4444", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let runs: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(
        runs["runs"][0]["platforms"]["linux-amd64"]["build_seconds"],
        json!(1114),
        "the machine answer carries the seconds, not a formatted string"
    );
    untouched(store.home.path());
}

#[test]
fn a_build_whose_job_the_queue_no_longer_holds_reports_no_cost() {
    let store = Store::new();
    store.seed_run(
        "eeee5555eeee5555eeee5555eeee5555",
        "lake",
        "0.2.5",
        "completed",
    );

    let out = store.stado(&["release", "status", "--run", "eeee5555"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("eeee5555 lake 0.2.5"), "{text}");
    assert!(
        !text.contains("took"),
        "a run whose job is gone invents no duration: {text}"
    );
    untouched(store.home.path());
}
