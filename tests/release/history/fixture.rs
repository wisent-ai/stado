//! What every release-history case stands on: a bare upstream holding one
//! base commit and a clone tracking it under a fresh directory of
//! `.build/release-history/`, fixture commits made with Git's plumbing, and
//! the real binary run with a plan, each run kept for the report.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

pub const NOTES: &str = "refs/notes/attestation";

pub fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {output:?}",
        root.display()
    );
    String::from_utf8(output.stdout)
        .expect("git prints UTF-8")
        .trim()
        .to_string()
}

/// A fixture commit of `files` on the branch HEAD names, made with git's
/// plumbing: the fixture's commits are the journey's input, not work any
/// commit standard of this machine reviews.
pub fn commit(root: &Path, files: &[(&str, &str)], message: &str) -> String {
    for (path, text) in files {
        fs::write(root.join(path), text).expect("write a fixture file");
        git(root, &["add", path]);
    }
    let tree = git(root, &["write-tree"]);
    // An unborn branch has no HEAD commit, and the first commit no parent.
    let parent = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .output()
        .expect("run git rev-parse");
    let parent = String::from_utf8(parent.stdout).expect("git prints UTF-8");
    let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
    if !parent.trim().is_empty() {
        args.extend(["-p", parent.trim()]);
    }
    let made = git(root, &args);
    git(root, &["update-ref", "HEAD", &made]);
    made
}

pub struct Journey {
    pub area: PathBuf,
    pub clone: PathBuf,
    pub log: Vec<Value>,
}

impl Journey {
    pub fn history(&mut self, plan: &Value) -> Output {
        let file = self.area.join(format!("plan-{}.json", self.log.len()));
        fs::write(&file, plan.to_string()).expect("write the plan");
        let args = [
            "release",
            "history",
            "--root",
            self.clone.to_str().expect("clone path"),
            "--plan",
            file.to_str().expect("plan path"),
            "--notes-ref",
            NOTES,
            "--json",
        ];
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .output()
            .expect("run stado release history");
        self.log.push(json!({
            "argv": args,
            "plan": plan,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        output
    }

    /// Keep every run, with the checkout's revision and the binary, in
    /// `report.json` beside the clone; the report, for assertion messages.
    pub fn report(&self, checkout: &Path) -> Value {
        let report = json!({
            "source_revision": git(checkout, &["rev-parse", "HEAD"]),
            "binary": env!("CARGO_BIN_EXE_stado"),
            "commands": self.log,
        });
        fs::write(
            self.area.join("report.json"),
            serde_json::to_vec_pretty(&report).expect("serialize the report"),
        )
        .expect("keep the report");
        eprintln!(
            "release history evidence: {}",
            self.area.join("report.json").display()
        );
        report
    }
}

pub struct Fixture {
    pub checkout: PathBuf,
    pub area: PathBuf,
    pub upstream: PathBuf,
    pub clone: PathBuf,
}

pub fn fixture() -> Fixture {
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("Stado checkout")
        .to_path_buf();
    let area = checkout
        .join(".build/release-history")
        .join(uuid::Uuid::new_v4().to_string());
    let upstream = area.join("upstream.git");
    let clone = area.join("clone");
    fs::create_dir_all(&upstream).expect("make the upstream");
    git(
        &upstream,
        &["init", "--quiet", "--bare", "--initial-branch=main"],
    );
    git(
        &area,
        &[
            "clone",
            "--quiet",
            upstream.to_str().expect("path"),
            "clone",
        ],
    );
    git(&clone, &["config", "user.name", "History Journey"]);
    git(&clone, &["config", "user.email", "history@example.invalid"]);
    // The clone of an empty upstream takes the machine's default branch name;
    // the journey's branch is main, tracking the upstream's.
    git(&clone, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    commit(&clone, &[("base.txt", "base\n")], "Base");
    // The upstream takes the base by fetching it, so no push hook of this
    // machine reviews the fixture; the clone then tracks it.
    git(
        &upstream,
        &[
            "fetch",
            "--quiet",
            clone.to_str().expect("path"),
            "main:main",
        ],
    );
    git(&clone, &["fetch", "--quiet", "origin"]);
    git(
        &clone,
        &["branch", "--quiet", "--set-upstream-to=origin/main", "main"],
    );
    Fixture {
        checkout,
        area,
        upstream,
        clone,
    }
}
