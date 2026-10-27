//! A wait on a resource that is not a lock file — a service, a product
//! revision — is an edge of the wait-for graph like a lock wait: one that
//! would close a cycle is refused naming every link, one without a cycle is
//! registered, and `stado waits cycles` names a cycle that formed after its
//! waits began.
//!
//! Every case runs the real `stado waits` against an isolated register under
//! Cargo's test directory. The processes are real: this test, and a `cat`
//! child reading a pipe the test holds, which stands in for an installation
//! whose converge waits. The child holds the service its converge needs
//! (`service:example-api`) and waits for the product revision that repairs
//! it (`revision:example@next`); this test holds that revision and waits on
//! the service.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use serde_json::Value;

const SERVICE: &str = "service:example-api";
const REVISION: &str = "revision:example@next";

struct Graph {
    root: PathBuf,
    child: Child,
}

impl Graph {
    fn new(case: &str) -> Self {
        let parent =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../.build/journeys/wait-cycle-resource");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create the case directory");
        let child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("start a child that waits on a pipe");
        Self { root, child }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .current_dir(&self.root)
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .env("STADO_WAITS_DIR", self.root.join("waits"))
            .stdin(Stdio::null())
            .output()
            .expect("run the real stado")
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("stado prints JSON")
    }

    fn hold(&self, resource: &str, pid: u32) -> String {
        let held = self.json(&[
            "waits",
            "hold",
            "--resource",
            resource,
            "--pid",
            &pid.to_string(),
            "--json",
        ]);
        assert_eq!(held["resource"], resource, "{held}");
        held["id"]
            .as_str()
            .expect("stado names the holding")
            .to_string()
    }

    fn open(&self, awaits: &str, pid: u32) -> Output {
        self.run(&[
            "waits",
            "open",
            "--kind",
            "siec",
            "--what",
            "converge",
            "--where",
            "the example API",
            "--pid",
            &pid.to_string(),
            "--awaits",
            awaits,
            "--json",
        ])
    }

    /// The child holds the service and waits for the revision.
    fn child_waits_for_the_revision(&self) {
        let child = self.child.id();
        self.hold(SERVICE, child);
        let opened = self.open(REVISION, child);
        assert!(
            opened.status.success(),
            "no one holds the revision yet, so the child's wait is registered: {}",
            String::from_utf8_lossy(&opened.stderr)
        );
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The rows of `section` in `document` that belong to process `pid`.
fn of(document: &Value, section: &str, pid: u32) -> Vec<Value> {
    let Some(rows) = document[section].as_array() else {
        panic!("the document has no {section} array: {document}");
    };
    rows.iter()
        .filter(|row| row["pid"] == pid)
        .cloned()
        .collect()
}

#[test]
fn a_wait_on_a_service_whose_repair_the_waiter_holds_is_refused_naming_the_cycle() {
    let graph = Graph::new("refused");
    graph.child_waits_for_the_revision();
    let me = std::process::id();
    graph.hold(REVISION, me);
    let refused = graph.open(SERVICE, me);
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a wait closing a cycle is refused"
    );
    for part in [
        "a wait cycle",
        &format!("process {me}"),
        "waits on service example-api",
        &format!("held by process {}", graph.child.id()),
        "waits on revision example@next",
        "none of these waits can end",
    ] {
        assert!(said.contains(part), "the refusal names {part:?}: {said}");
    }
    let listing = graph.json(&["waits", "--json"]);
    assert!(
        of(&listing, "open", me).is_empty(),
        "the refused wait was not registered: {listing}"
    );
    assert!(
        !of(&listing, "open", graph.child.id()).is_empty(),
        "the child's wait stands: {listing}"
    );
}

#[test]
fn a_wait_on_a_service_without_a_cycle_is_registered() {
    let graph = Graph::new("registered");
    graph.child_waits_for_the_revision();
    let opened = graph.open(SERVICE, std::process::id());
    assert!(
        opened.status.success(),
        "nothing this test needs holds the revision: {}",
        String::from_utf8_lossy(&opened.stderr)
    );
    let answer: Value = serde_json::from_slice(&opened.stdout).expect("stado prints JSON");
    assert_eq!(answer["awaits"], SERVICE, "{answer}");
    let cycles = graph.json(&["waits", "cycles", "--json"]);
    assert_eq!(cycles["cycles"], Value::Array(Vec::new()), "{cycles}");
}

#[test]
fn a_cycle_formed_after_its_waits_began_is_listed_and_ends_with_its_holding() {
    let graph = Graph::new("standing");
    graph.child_waits_for_the_revision();
    let me = std::process::id();
    let opened = graph.open(SERVICE, me);
    assert!(opened.status.success(), "no cycle stands yet");
    let holding = graph.hold(REVISION, me);
    let standing = graph.run(&["waits", "cycles", "--json"]);
    assert!(
        !standing.status.success(),
        "a standing cycle fails the reading"
    );
    let document: Value = serde_json::from_slice(&standing.stdout).expect("stado prints JSON");
    let Some(cycles) = document["cycles"].as_array() else {
        panic!("the standing cycles are listed: {document}");
    };
    let through_both = cycles.iter().any(|cycle| {
        let Some(links) = cycle["links"].as_array() else {
            panic!("a listed cycle names its links: {cycle}");
        };
        let awaited: Vec<&Value> = links.iter().map(|link| &link["awaits"]).collect();
        awaited.contains(&&Value::from(SERVICE)) && awaited.contains(&&Value::from(REVISION))
    });
    assert!(
        through_both,
        "a cycle through the service and the revision: {document}"
    );
    let released = graph.json(&["waits", "release", &holding, "--json"]);
    assert_eq!(released["removed"], true, "{released}");
    let after = graph.json(&["waits", "cycles", "--json"]);
    assert_eq!(
        after["cycles"],
        Value::Array(Vec::new()),
        "released: {after}"
    );
    let holds = graph.json(&["waits", "holds", "--json"]);
    assert!(
        of(&holds, "holds", me).is_empty(),
        "this test holds nothing: {holds}"
    );
    let child = of(&holds, "holds", graph.child.id());
    assert!(
        child.iter().any(|holding| holding["resource"] == SERVICE),
        "the child still holds the service: {holds}"
    );
}

#[test]
fn a_resource_that_is_no_resource_is_refused() {
    let graph = Graph::new("malformed");
    let me = std::process::id().to_string();
    let unknown = graph.run(&["waits", "hold", "--resource", "disk:sda", "--pid", &me]);
    let said = String::from_utf8_lossy(&unknown.stderr);
    assert!(!unknown.status.success(), "an unknown kind is refused");
    assert!(
        said.contains("lock, host, service, revision, unit, port"),
        "the refusal names the kinds: {said}"
    );
    let bare = graph.open("example-api", std::process::id());
    assert!(
        !bare.status.success(),
        "a resource without a kind is refused"
    );
    assert!(String::from_utf8_lossy(&bare.stderr).contains("<kind>:<id>"));
    let release = graph.run(&["waits", "release", "not-an-id/../x"]);
    assert!(!release.status.success(), "a path is not a holding id");
    assert!(String::from_utf8_lossy(&release.stderr).contains("is not a holding id"));
}
