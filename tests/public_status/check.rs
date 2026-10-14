//! `stado public-status check` asks every component the catalog declares and
//! publishes what each answered; `show` reads it back.
//!
//! One isolated deployment: a local object store and a home of its own. The
//! built stado is asked, against the real endpoints the compiled catalog
//! declares (`public_status` of each product):
//!
//! - `show` before any check is refused, naming that nothing is published;
//! - `check --json` publishes one row per declared component, each `up` or
//!   `down` with the answer that says so, `since` the first check, and a down
//!   row's `last_failure` carrying that check's error;
//! - a second check keeps `since` for every component whose state did not
//!   change, and `show --json` answers exactly what the last check published;
//! - the plain `show` names every component on its own line.
//!
//! A component this deployment cannot reach (a vault field, a forward marker
//! this home does not hold) is reported down with that cause: the test
//! requires each row to say why, not that the fleet is healthy.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Deployment {
    root: PathBuf,
    report: Value,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/public-status-check")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store")).unwrap();
        Self {
            root,
            report: json!({
                "source_revision": std::env::var("STADO_SOURCE_REVISION")
                    .expect("declare the committed candidate"),
                "commands": [],
                "outcome": "failed",
            }),
        }
    }

    fn run(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn json(&mut self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(output.status.success(), "stado {args:?}: {output:?}");
        serde_json::from_slice(&output.stdout).expect("the answer is JSON")
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

/// Every `(product, component)` the compiled catalog declares.
fn declared() -> Vec<(String, String)> {
    let catalog = stado_product::catalog::embedded().expect("the compiled catalog is valid");
    let mut declared = Vec::new();
    for product in catalog["products"].as_array().unwrap() {
        for check in product["public_status"].as_array().into_iter().flatten() {
            declared.push((
                product["id"].as_str().unwrap().to_owned(),
                check["component"].as_str().unwrap().to_owned(),
            ));
        }
    }
    assert!(!declared.is_empty(), "the catalog declares components");
    declared
}

fn row<'a>(document: &'a Value, product: &str, component: &str) -> &'a Value {
    let Some(row) = document["components"].as_array().and_then(|rows| {
        rows.iter()
            .find(|row| row["product"] == product && row["component"] == component)
    }) else {
        panic!("{product}/{component} is published: {document}");
    };
    row
}

/// A row of the first check: down with its failure, or up with none.
fn first_look(row: &Value, checked_at: &str) {
    assert_eq!(row["since"], checked_at, "{row}");
    assert!(
        row["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()),
        "the answer that says so: {row}"
    );
    if row["state"] == "down" {
        assert_eq!(row["last_failure"]["at"], checked_at, "{row}");
        assert_eq!(row["last_failure"]["error"], row["detail"], "{row}");
        return;
    }
    assert_eq!(row["state"], "up", "{row}");
    assert!(row["last_failure"].is_null(), "{row}");
}

#[test]
fn every_declared_component_is_checked_published_and_read_back() {
    let mut deployment = Deployment::start();
    let init = deployment.run(&["config", "init"]);
    assert!(init.status.success(), "{init:?}");

    let unpublished = deployment.run(&["public-status", "show", "--json"]);
    assert!(!unpublished.status.success(), "{unpublished:?}");
    assert!(
        String::from_utf8_lossy(&unpublished.stderr).contains("no status is published"),
        "{unpublished:?}"
    );

    let first = deployment.json(&["public-status", "check", "--json"]);
    let checked_at = first["checked_at"]
        .as_str()
        .expect("the check is dated")
        .to_owned();
    for (product, component) in declared() {
        first_look(row(&first, &product, &component), &checked_at);
    }

    let second = deployment.json(&["public-status", "check", "--json"]);
    for (product, component) in declared() {
        let before = row(&first, &product, &component);
        let after = row(&second, &product, &component);
        if before["state"] == after["state"] {
            assert_eq!(after["since"], before["since"], "{before} then {after}");
        } else {
            assert_eq!(
                after["since"], second["checked_at"],
                "{before} then {after}"
            );
        }
    }

    let shown = deployment.json(&["public-status", "show", "--json"]);
    assert_eq!(shown, second);

    let plain = deployment.run(&["public-status", "show"]);
    assert!(plain.status.success(), "{plain:?}");
    let plain = String::from_utf8_lossy(&plain.stdout).into_owned();
    for (product, component) in declared() {
        assert!(
            plain.contains(&format!("({product}/{component})")),
            "{product}/{component} is named: {plain}"
        );
    }
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
