//! A commit is checked for the values this installation declares about
//! itself before anything else of it is read: a registry target's name, its
//! host names and its ssh login, this account's home directory and name.
//! The pre-push check refuses a commit that carries one in an added line or
//! in its message, naming each place and what declares it; a commit without
//! one passes; a registry that cannot be read refuses rather than passing
//! every commit.
//!
//! Each case is a real Git repository under Cargo's test directory with an
//! isolated `HOME` and a local object store holding its registry, read by
//! the real `stado`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::json;

const HOST: &str = "quality-journey-host";
const HOSTNAME: &str = "quality-journey-host.example";
const LOGIN_ACCOUNT: &str = "quality-journey-login";
const LOGIN_HOST: &str = "quality-journey-ssh.example";
const PERSON: &str = "quality-journey-person";
const REFUSED: &str = "carries this installation's own values, which a commit must not hold";

struct Checkout {
    base: PathBuf,
    root: PathBuf,
    home: PathBuf,
    store: PathBuf,
}

impl Checkout {
    fn new(case: &str, registry: bool) -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/quality-instance-values")
            .join(format!("{case}-{}", std::process::id()));
        let root = base.join("checkout");
        let home = base.join(PERSON);
        let store = base.join("store");
        for directory in [&root, &home, &store] {
            std::fs::create_dir_all(directory).expect("create the journey's directories");
        }
        if registry {
            let mut document: serde_json::Value =
                serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
            let target = &mut document["targets"][0];
            target["name"] = json!(HOST);
            target["hostnames"] = json!([HOSTNAME]);
            target["ssh"] = json!(format!("{LOGIN_ACCOUNT}@{LOGIN_HOST}"));
            std::fs::write(
                store.join("registry.json"),
                serde_json::to_vec_pretty(&document).unwrap(),
            )
            .expect("write the registry");
        }
        let checkout = Self {
            base,
            root,
            home,
            store,
        };
        checkout.git(&["init", "--quiet", "--initial-branch", "main"]);
        checkout
    }

    fn commit(&self, file: &str, text: &str, message: &str) -> String {
        std::fs::write(self.root.join(file), text).expect("write the committed file");
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.root)
            .env("GIT_AUTHOR_NAME", "Quality Journey")
            .env("GIT_AUTHOR_EMAIL", "quality-journey@example.invalid")
            .env("GIT_COMMITTER_NAME", "Quality Journey")
            .env("GIT_COMMITTER_EMAIL", "quality-journey@example.invalid")
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout)
            .expect("git output is UTF-8")
            .trim()
            .to_owned()
    }

    /// The pre-push check of `revision` pushed to a new branch, as a Git
    /// dispatcher runs it (a push to main also asks the submit queue about
    /// origin's main, which this repository has none of).
    fn pushed(&self, revision: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env("HOME", &self.home)
            .env(
                "STADO_CONFIG",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/config/empty.json"),
            )
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.store)
            .args(["quality", "check", "--root"])
            .arg(&self.root)
            .args(["--pre-push", "--if-declared"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run the real stado");
        let update = format!(
            "refs/heads/main {revision} refs/heads/journey-candidate {}\n",
            "0".repeat(40)
        );
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(update.as_bytes())
            .expect("write the update");
        child.wait_with_output().expect("stado ends")
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_commit_without_this_installations_values_passes() {
    let checkout = Checkout::new("clean", true);
    let head = checkout.commit(
        "notes.txt",
        "A general rule for every host.\n",
        "Describe the general rule",
    );
    let answer = checkout.pushed(&head);
    assert!(
        answer.status.success(),
        "a clean commit passes: {}",
        said(&answer)
    );
    assert!(!said(&answer).contains(REFUSED), "{}", said(&answer));
}

#[test]
fn a_host_name_in_an_added_line_is_refused_naming_its_place() {
    let checkout = Checkout::new("line", true);
    let head = checkout.commit(
        "notes.txt",
        &format!("first line\nconnect to {HOSTNAME} first\n"),
        "Describe the connection",
    );
    let answer = checkout.pushed(&head);
    let text = said(&answer);
    assert!(!answer.status.success(), "{text}");
    assert!(text.contains(REFUSED), "{text}");
    assert!(
        text.contains(&format!(
            "notes.txt:2: `{HOSTNAME}` (registry target {HOST}'s host name)"
        )),
        "{text}"
    );
}

#[test]
fn a_login_and_a_home_directory_are_refused_in_a_message_and_a_line() {
    let checkout = Checkout::new("message", true);
    let home = checkout.home.display().to_string();
    let head = checkout.commit(
        "paths.txt",
        &format!("cache under {home}/cache\n"),
        &format!("Repair what {LOGIN_ACCOUNT} saw"),
    );
    let answer = checkout.pushed(&head);
    let text = said(&answer);
    assert!(!answer.status.success(), "{text}");
    assert!(
        text.contains(&format!(
            "commit message: `{LOGIN_ACCOUNT}` (registry target {HOST}'s ssh login)"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "paths.txt:1: `{home}` (this account's home directory)"
        )),
        "{text}"
    );
}

#[test]
fn an_unreadable_registry_refuses_instead_of_passing() {
    let checkout = Checkout::new("unread", false);
    let head = checkout.commit("notes.txt", "A general rule.\n", "Describe it");
    let answer = checkout.pushed(&head);
    let text = said(&answer);
    assert!(!answer.status.success(), "{text}");
    assert!(
        text.contains("this installation's own values cannot be read"),
        "{text}"
    );
}
