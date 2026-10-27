//! A host's real resolver waiting list is classified by its declared refresh
//! interval, and only answer-phase waits on another host become standings.
//!
//! The beacon pass reads the waiting list this host's resolver publishes
//! beside its state (`STADO_RESOLVER_STATE_FILE`, its sibling
//! `resolver-waiting.json`), judged by the refresh interval that resolver
//! publishes. The run writes both as a resolver publishes them, under an
//! isolated home in the checkout's build directory, with this test process
//! as the publisher, and calls the pass the beacon calls. What it proves:
//!
//! - a standing service the catalog does not declare is not acted on, and
//!   the reason names it;
//! - a wait still opening its channel, a wait on this host's own service,
//!   and a wait younger than the bound are not standings at all;
//! - a list whose publisher no longer runs names nothing, and a list that
//!   cannot be read is said as unreadable.
//!
//! The resolver-list journey does not prove recovery of a managed service on
//! another host. That requires a real host and its declared repair and restart
//! over the registry channel. The report keeps the source revision and answers.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use stado::deploy::service_answering::peer::recoveries;

const THIS_HOST: &str = "marker-host";
const OTHER_HOST: &str = "other-host";
/// A catalog-declared service used for a young wait, not a recovery.
const DECLARED: &str = "brama";
/// A service no catalog declares.
const UNDECLARED: &str = "peer-journey-undeclared";
const LONG_AGO: &str = "1970-01-01T00:00:00Z";
const ANSWER: &str = "answer";
const OPEN: &str = "open";

/// The refresh interval the fixture registry declares for its resolver.
fn bound_seconds() -> u64 {
    let registry: Value =
        serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
    registry["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|target| target["service_resolver"]["refresh_seconds"].as_u64())
        .expect("the fixture registry declares a refresh interval")
}

/// One wait as the resolver publishes it. The addresses are names under the
/// reserved example domain: the pass reads them only to say them.
fn wait(service: &str, host: &str, since: &str, phase: &str, client: &str) -> Value {
    json!({
        "service": service,
        "consumer": "peer-journey",
        "bind": "adapter.example",
        "active_host": host,
        "endpoint": "service.example",
        "since": since,
        "phase": phase,
        "client": client,
        "request": format!("GET /{service} HTTP/1.1"),
    })
}

/// A text field of an answer, or a failure naming the field and the answer.
fn text<'a>(answer: &'a Value, field: &str) -> &'a str {
    match answer[field].as_str() {
        Some(text) => text,
        None => panic!("the answer carries no text {field}: {answer}"),
    }
}

struct Run {
    home: PathBuf,
    report: Value,
}

impl Run {
    fn new() -> Self {
        let home = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/service-answering-peer")
            .join(std::process::id().to_string());
        if home.exists() {
            std::fs::remove_dir_all(&home).expect("a previous run's home is removed");
        }
        std::fs::create_dir_all(home.join(".stado")).expect("the isolated home is made");
        // The pass reads the resolver's files from here; nothing of this
        // account's own resolver is read.
        std::env::set_var(
            "STADO_RESOLVER_STATE_FILE",
            home.join(".stado/resolver-state.json"),
        );
        let revision = Command::new("git")
            .arg("-C")
            .arg(env!("CARGO_MANIFEST_DIR"))
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git answers the checkout's revision");
        Self {
            report: json!({
                "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
                "answers": [],
                "outcome": "failed",
            }),
            home,
        }
    }

    fn publish(&self, publisher: u32, waits: &Value) {
        std::fs::write(
            self.home.join(".stado/resolver-state.json"),
            serde_json::to_vec(&json!({
                "target": THIS_HOST,
                "pid": publisher,
                "state": "serving",
                "listening": true,
                "refresh_seconds": bound_seconds(),
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            self.home.join(".stado/resolver-waiting.json"),
            serde_json::to_vec(waits).unwrap(),
        )
        .unwrap();
    }

    async fn pass(&mut self, case: &str) -> Value {
        let answer = recoveries(&self.home, THIS_HOST).await;
        self.report["answers"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "case": case, "answer": answer }));
        self.save();
        answer
    }

    fn save(&self) {
        std::fs::write(
            self.home.with_extension("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn another_hosts_answer_waits_are_classified_without_touching_a_service() {
    let mut run = Run::new();
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let waits = vec![
        wait(DECLARED, OTHER_HOST, &now, ANSWER, "young-client"),
        wait(
            UNDECLARED,
            OTHER_HOST,
            LONG_AGO,
            ANSWER,
            "undeclared-client",
        ),
        wait("skarbiec", OTHER_HOST, LONG_AGO, OPEN, "opening-client"),
        wait("skarbiec", THIS_HOST, LONG_AGO, ANSWER, "own-client"),
    ];
    run.publish(std::process::id(), &Value::Array(waits));
    let answer = run.pass("published by a running resolver").await;
    let blocks = answer
        .as_array()
        .expect("the pass answers one block per standing");
    let services: Vec<(&str, &str)> = blocks
        .iter()
        .map(|block| (text(block, "service"), text(block, "host")))
        .collect();
    assert_eq!(
        services,
        vec![(UNDECLARED, OTHER_HOST)],
        "only the answer-phase waits to another host past the bound are standings: {answer}"
    );
    let [undeclared] = blocks.as_slice() else {
        panic!("one undeclared standing was read: {answer}");
    };

    let reason = text(undeclared, "not_recovered");
    assert!(
        reason.contains(UNDECLARED) && reason.contains("is not a service the catalog declares"),
        "an undeclared service is not acted on, naming it: {undeclared}"
    );

    let mut gone = Command::new("/usr/bin/true")
        .spawn()
        .expect("a process is started");
    let gone_pid = gone.id();
    gone.wait().expect("the process ends");
    run.publish(
        gone_pid,
        &json!([wait(DECLARED, OTHER_HOST, LONG_AGO, ANSWER, "first-client")]),
    );
    let answer = run
        .pass("published by a resolver that no longer runs")
        .await;
    assert_eq!(
        answer,
        json!([]),
        "waits whose resolver ended went with it and name nothing: {answer}"
    );

    run.publish(std::process::id(), &json!({ "not": "a list" }));
    let answer = run.pass("a waiting list that is not a list").await;
    assert!(
        text(&answer, "unreadable").contains("resolver-waiting.json"),
        "a list that cannot be read is said, naming the file: {answer}"
    );

    run.report["outcome"] = json!("passed");
    run.save();
    std::fs::remove_dir_all(&run.home).expect("the isolated home is removed");
}
