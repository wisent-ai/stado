//! `stado release staged activate` takes the settle window from the operator:
//! a missing `--settle-seconds` and a window of zero are refused by name
//! through the built binary, before any host is resolved, and nothing is
//! written. HOME, the store and the config are the test's own directories,
//! so no operator state is read or written and no host is contacted.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("staged-settle-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("store")).expect("create the store");
        std::fs::create_dir_all(root.join("home")).expect("create the home");
        std::fs::write(root.join("config.json"), "{}").expect("write the empty config");
        Self { root }
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("STADO_CONFIG", self.root.join("config.json"))
            .env("HOME", self.root.join("home"))
            .output()
            .expect("the stado binary runs")
    }

    /// Every file under the fixture, so a refusal is shown to write nothing.
    fn files(&self) -> Vec<PathBuf> {
        fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
            let entries = std::fs::read_dir(directory).expect("read a fixture directory");
            for entry in entries {
                let path = entry.expect("read a fixture entry").path();
                if path.is_dir() {
                    walk(&path, found);
                } else {
                    found.push(path);
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.root, &mut found);
        found.sort();
        found
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const ACTIVATE: [&str; 11] = [
    "release",
    "staged",
    "activate",
    "--host",
    "example-host",
    "--product",
    "weles-worker",
    "--env-file",
    "/etc/example/weles.env",
    "--port",
    "4000",
];

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_missing_settle_window_is_refused_by_name_and_writes_nothing() {
    let fixture = Fixture::new("missing");
    let before = fixture.files();
    let output = fixture.stado(&ACTIVATE);
    let said = stderr(&output);
    assert_eq!(output.status.code(), Some(2), "exit status; stderr: {said}");
    assert!(
        said.contains("--settle-seconds"),
        "the refusal names the missing argument: {said}"
    );
    assert_eq!(fixture.files(), before, "a refused activation wrote a file");
}

#[test]
fn a_zero_settle_window_is_refused_by_name_and_writes_nothing() {
    let fixture = Fixture::new("zero");
    let before = fixture.files();
    let mut args = ACTIVATE.to_vec();
    args.extend(["--settle-seconds", "0"]);
    let output = fixture.stado(&args);
    let said = stderr(&output);
    assert_eq!(output.status.code(), Some(2), "exit status; stderr: {said}");
    assert!(
        said.contains("--settle-seconds"),
        "the refusal names the argument whose value is refused: {said}"
    );
    assert_eq!(fixture.files(), before, "a refused activation wrote a file");
}
