//! A wait a program does not perform in Rust itself is registered for the
//! waiting process with `stado waits open`, read like any other, and ended
//! by `stado waits close` or by its process ending.
//!
//! The real `stado waits` runs with an isolated register under Cargo's test
//! directory. A `zgoda` wait registered for this test's own process is
//! listed as open with its line (`rodzaj: zgoda`, its what and where);
//! `stado waits close` removes it, the next listing no longer holds it, and
//! closing it again answers that the register no longer held it. A
//! `terminal` wait registered for a child the test starts (`cat` reading a
//! pipe the test holds) is reported ended once that child is killed. The
//! refusals: a kind the register does not know (naming the kinds it does),
//! an empty --what, and a close of something that is not a wait id.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Register {
    root: PathBuf,
}

impl Register {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("wait-registered");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create the case directory");
        Self { root }
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

    fn open(&self, kind: &str, what: &str, place: &str, pid: u32) -> String {
        let pid = pid.to_string();
        let opened = self.json(&[
            "waits", "open", "--kind", kind, "--what", what, "--where", place, "--pid", &pid,
            "--json",
        ]);
        assert_eq!(opened["kind"], kind, "{opened}");
        opened["id"]
            .as_str()
            .expect("stado names the registered wait")
            .to_string()
    }
}

impl Drop for Register {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn held<'a>(listing: &'a Value, section: &str, id: &str) -> Option<&'a Value> {
    let Some(waits) = listing[section].as_array() else {
        panic!("the listing has no {section} array: {listing}");
    };
    waits.iter().find(|wait| wait["id"] == id)
}

#[test]
fn a_registered_wait_is_listed_and_closed() {
    let register = Register::new("closed");
    let id = register.open(
        "zgoda",
        "build:stado",
        "the harness's grant dialog",
        std::process::id(),
    );
    let listing = register.json(&["waits", "--json"]);
    let wait = held(&listing, "open", &id).expect("the registered wait is open");
    let line = wait["line"].as_str().expect("the wait holds its line");
    for field in [
        "czekam: build:stado",
        "gdzie: the harness's grant dialog",
        "rodzaj: zgoda",
    ] {
        assert!(line.contains(field), "{line} carries {field}");
    }
    let closed = register.json(&["waits", "close", &id, "--json"]);
    assert_eq!(closed["removed"], true, "{closed}");
    let after = register.json(&["waits", "--json"]);
    assert!(
        held(&after, "open", &id).is_none(),
        "a closed wait is gone: {after}"
    );
    let again = register.json(&["waits", "close", &id, "--json"]);
    assert_eq!(
        again["removed"], false,
        "closing it again says it was gone: {again}"
    );
}

#[test]
fn a_registered_wait_ends_with_its_process() {
    let register = Register::new("ended");
    let mut child = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("start a child that reads a pipe the test holds");
    let id = register.open("terminal", "cat", "a pipe nobody writes", child.id());
    let listing = register.json(&["waits", "--json"]);
    assert!(
        held(&listing, "open", &id).is_some(),
        "open while its process runs: {listing}"
    );
    child.kill().expect("kill the child");
    child.wait().expect("reap the child");
    let after = register.json(&["waits", "--json"]);
    assert!(
        held(&after, "ended", &id).is_some(),
        "ended once its process is gone: {after}"
    );
}

#[test]
fn a_wait_the_register_cannot_hold_is_refused_naming_why() {
    let register = Register::new("refused");
    let own = std::process::id().to_string();
    let kind = register.run(&[
        "waits", "open", "--kind", "keyboard", "--what", "x", "--where", "y", "--pid", &own,
    ]);
    let said = String::from_utf8_lossy(&kind.stderr);
    assert!(!kind.status.success(), "an unknown kind is refused");
    assert!(
        said.contains("keyboard") && said.contains("terminal") && said.contains("zgoda"),
        "the refusal names the kind and the kinds there are: {said}"
    );
    let empty = register.run(&[
        "waits", "open", "--kind", "terminal", "--what", " ", "--where", "y", "--pid", &own,
    ]);
    assert!(!empty.status.success(), "an empty --what is refused");
    assert!(String::from_utf8_lossy(&empty.stderr).contains("--what"));
    let malformed = register.run(&["waits", "close", "not-an-id/../x"]);
    assert!(!malformed.status.success(), "a path is not a wait id");
    assert!(String::from_utf8_lossy(&malformed.stderr).contains("is not a wait id"));
}
