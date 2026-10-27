//! `stado schedule list --json` lists every schedule a store holds exactly
//! once, and no removed one, whichever order its reads finish in: the
//! listing reads its schedules side by side.
//!
//! The real `stado` runs against an isolated local store under the build
//! directory; one schedule is declared for each cron expression below with
//! `stado schedule create --id`, the last is removed with `stado schedule
//! rm`, and the listing is read back. Every command, its exit status and
//! its output are kept in the case's report.json, with the outcome.
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

/// The cron expressions of the schedules the journey declares, one each.
const CRONS: &[&str] = &["* * * * *", "*/5 * * * *", "0 * * * *", "0 0 * * *"];

struct Store {
    root: PathBuf,
    report: RefCell<Value>,
}

impl Store {
    fn new() -> Self {
        let parent =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../.build/journeys/schedule-listing");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(uuid::Uuid::new_v4().to_string());
        for directory in [".locks", ".metadata"] {
            std::fs::create_dir_all(root.join("store").join(directory))
                .expect("lay out the isolated local store");
        }
        let report = RefCell::new(json!({ "commands": [], "outcome": "failed" }));
        Self { root, report }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .current_dir(&self.root)
            .env("HOME", &self.root)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run the real stado");
        self.report.borrow_mut()["commands"]
            .as_array_mut()
            .expect("the report keeps a command list")
            .push(json!({
                "args": arguments,
                "exit_status": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            }));
        self.save();
        output
    }

    fn succeeds(&self, arguments: &[&str]) -> Output {
        let output = self.run(arguments);
        assert!(
            output.status.success(),
            "stado {}: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn save(&self) {
        let written =
            serde_json::to_vec_pretty(&*self.report.borrow()).expect("the report is JSON");
        std::fs::write(self.root.join("report.json"), written).expect("keep the case's report");
    }
}

#[test]
fn every_declared_schedule_is_listed_once_and_a_removed_one_is_not() {
    let store = Store::new();
    let mut declared = Vec::new();
    for cron in CRONS {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let command = format!("echo {id}");
        store.succeeds(&["schedule", "create", "--id", &id, "--cron", cron, &command]);
        declared.push(id);
    }
    let removed = declared.last().expect("the journey declares schedules");
    store.succeeds(&["schedule", "rm", removed]);

    let listed = store.succeeds(&["schedule", "list", "--json"]);
    let schedules: Value =
        serde_json::from_slice(&listed.stdout).expect("schedule list --json prints JSON");
    let ids: Vec<&str> = schedules
        .as_array()
        .expect("schedule list --json prints a list")
        .iter()
        .map(|schedule| {
            schedule["schedule_id"]
                .as_str()
                .expect("each schedule names its id")
        })
        .collect();
    let unique: BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "a schedule was listed twice: {ids:?}"
    );
    let expected: BTreeSet<&str> = declared
        .iter()
        .map(String::as_str)
        .filter(|id| *id != removed.as_str())
        .collect();
    assert_eq!(
        unique, expected,
        "the listing is not every declared schedule but the removed one"
    );
    store.report.borrow_mut()["outcome"] = json!("passed");
    store.save();
    eprintln!("schedule-listing evidence: {}", store.root.display());
}
