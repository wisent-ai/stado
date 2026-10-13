//! `stado schedule create --id` declares a schedule under an identity its
//! caller keeps. The same declaration again changes nothing; a different one
//! is refused naming what differs; with `--converge` the standing schedule is
//! brought to the new declaration in place, keeping when it was created; a
//! removed schedule is not raised again by `--converge`.
//!
//! The real `stado` runs against an isolated local store under the build
//! directory, and every verdict is read back with `stado schedule show`.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const CRON: &str = "* * * * *";
const OLDER: &str = "echo older declaration";
const NEWER: &str = "echo newer declaration";

struct Store {
    root: PathBuf,
    id: String,
}

impl Store {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("schedule-declaration");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        for directory in [".locks", ".metadata"] {
            std::fs::create_dir_all(root.join("store").join(directory))
                .expect("lay out the isolated local store");
        }
        let id = uuid::Uuid::new_v4().to_string();
        Self { root, id }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
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
            .expect("run the real stado")
    }

    fn declare(&self, command: &str, converge: bool) -> Output {
        let mut arguments = vec!["schedule", "create", "--id", &self.id, "--cron", CRON];
        if converge {
            arguments.push("--converge");
        }
        arguments.push(command);
        self.run(&arguments)
    }

    fn shown(&self) -> Value {
        let output = self.run(&["schedule", "show", &self.id, "--json"]);
        assert!(
            output.status.success(),
            "schedule show: {}",
            stderr(&output)
        );
        serde_json::from_slice(&output.stdout).expect("schedule show prints JSON")
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_different_declaration_is_refused_and_converged_only_when_asked() {
    let store = Store::new("converge");
    let created = store.declare(OLDER, false);
    assert!(created.status.success(), "create: {}", stderr(&created));
    let first = store.shown();
    assert_eq!(first["command"], OLDER);

    let repeated = store.declare(OLDER, false);
    assert!(repeated.status.success(), "repeat: {}", stderr(&repeated));
    assert!(stdout(&repeated).contains("already declared as asked"));

    let refused = store.declare(NEWER, false);
    assert!(
        !refused.status.success(),
        "a different declaration must be refused"
    );
    let said = stderr(&refused);
    assert!(said.contains("with a different command"), "refusal: {said}");
    assert!(
        said.contains("--converge"),
        "refusal names the way forward: {said}"
    );
    assert_eq!(store.shown()["command"], OLDER, "a refusal changes nothing");

    let converged = store.declare(NEWER, true);
    assert!(
        converged.status.success(),
        "converge: {}",
        stderr(&converged)
    );
    assert!(
        stdout(&converged).contains("changed command"),
        "{}",
        stdout(&converged)
    );
    let after = store.shown();
    assert_eq!(after["command"], NEWER);
    assert_eq!(
        after["created_at"], first["created_at"],
        "its history is kept"
    );
    assert_eq!(after["created_by"], first["created_by"]);

    let again = store.declare(NEWER, true);
    assert!(again.status.success(), "converge again: {}", stderr(&again));
    assert!(stdout(&again).contains("already declared as asked"));
}

#[test]
fn a_removed_schedule_is_not_raised_again_by_converge() {
    let store = Store::new("removed");
    let created = store.declare(OLDER, false);
    assert!(created.status.success(), "create: {}", stderr(&created));
    let removed = store.run(&["schedule", "rm", &store.id]);
    assert!(removed.status.success(), "rm: {}", stderr(&removed));

    let refused = store.declare(NEWER, true);
    assert!(
        !refused.status.success(),
        "a removed schedule must stay removed"
    );
    let said = stderr(&refused);
    assert!(said.contains("as a deleted record"), "refusal: {said}");
}

#[test]
fn converge_without_an_identity_is_refused_by_the_cli() {
    let store = Store::new("unnamed");
    let refused = store.run(&["schedule", "create", "--cron", CRON, "--converge", OLDER]);
    assert!(!refused.status.success(), "--converge needs --id");
    assert!(stderr(&refused).contains("--id"), "{}", stderr(&refused));
}
