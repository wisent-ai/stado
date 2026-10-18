//! `stado release history` rewrites a real checkout's unpushed commits.
//!
//! A bare upstream and a clone are made under `.build/release-history/`. The
//! clone gets five unpushed commits: a feature, a commit that only reformats
//! it, one whose message is wrong, one that mixes two changes, and one that
//! carries a note. The real binary folds the format commit into the feature,
//! rewords the third, splits the fourth by path and keeps the fifth, and the
//! journey reads the clone back: the branch holds the new commits with the
//! planned messages, the tree is the old HEAD's, the working tree is clean,
//! only the unaltered commit carries its note, and the upstream is untouched.
//! Two refusals leave the branch where it was: a plan that leaves out a
//! commit, and a plan whose drop would change the tree. Every command, its
//! status and both streams are kept in `report.json` beside the clone.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const NOTES: &str = "refs/notes/attestation";

fn git(root: &Path, args: &[&str]) -> String {
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
fn commit(root: &Path, files: &[(&str, &str)], message: &str) -> String {
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

struct Journey {
    area: PathBuf,
    clone: PathBuf,
    log: Vec<Value>,
}

impl Journey {
    fn history(&mut self, plan: &Value) -> Output {
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
}

#[test]
fn a_plan_folds_rewords_splits_and_keeps_notes_and_a_bad_plan_moves_nothing() {
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
    let pushed = git(&upstream, &["rev-parse", "main"]);

    let feature = commit(
        &clone,
        &[("feature.txt", "feature  one\n")],
        "Add the feature",
    );
    let format = commit(
        &clone,
        &[("feature.txt", "feature one\n")],
        "Format the feature",
    );
    let misnamed = commit(&clone, &[("named.txt", "named\n")], "Says something else");
    let mixed = commit(
        &clone,
        &[("left.txt", "left\n"), ("right.txt", "right\n")],
        "Two",
    );
    let noted = commit(&clone, &[("noted.txt", "noted\n")], "Carries a note");
    git(
        &clone,
        &["notes", "--ref", NOTES, "add", "-m", "attested", &noted],
    );
    let head = git(&clone, &["rev-parse", "HEAD"]);
    let tree = git(&clone, &["rev-parse", "HEAD^{tree}"]);
    for (name, text) in [
        ("reworded", "Add the named file\n"),
        ("left", "Add left\n"),
        ("right", "Add right\n"),
    ] {
        fs::write(area.join(name), text).expect("write a message");
    }
    let mut journey = Journey {
        area: area.clone(),
        clone: clone.clone(),
        log: Vec::new(),
    };

    let incomplete = journey.history(&json!({"commits": [
        {"action": "pick", "commit": feature},
    ]}));
    let unchanged = git(&clone, &["rev-parse", "HEAD"]);
    let dropped = journey.history(&json!({"commits": [
        {"action": "pick", "commit": feature},
        {"action": "fold", "commit": format, "into": feature},
        {"action": "drop", "commit": misnamed},
        {"action": "pick", "commit": mixed},
        {"action": "pick", "commit": noted},
    ]}));
    let still = git(&clone, &["rev-parse", "HEAD"]);
    let rewritten = journey.history(&json!({"commits": [
        {"action": "pick", "commit": feature},
        {"action": "fold", "commit": format, "into": feature},
        {"action": "reword", "commit": misnamed, "message": area.join("reworded")},
        {"action": "split", "commit": mixed, "parts": [
            {"paths": ["left.txt"], "message": area.join("left")},
            {"paths": ["right.txt"], "message": area.join("right")},
        ]},
        {"action": "pick", "commit": noted},
    ]}));
    let report = json!({
        "source_revision": git(&checkout, &["rev-parse", "HEAD"]),
        "binary": env!("CARGO_BIN_EXE_stado"),
        "commands": journey.log,
    });
    fs::write(
        area.join("report.json"),
        serde_json::to_vec_pretty(&report).expect("serialize the report"),
    )
    .expect("keep the report");
    eprintln!(
        "release history evidence: {}",
        area.join("report.json").display()
    );

    assert!(!incomplete.status.success(), "{report}");
    assert!(
        String::from_utf8_lossy(&incomplete.stderr).contains("must name every unpushed commit"),
        "{report}"
    );
    assert_eq!(unchanged, head, "a refused plan moved the branch");
    assert!(!dropped.status.success(), "{report}");
    assert!(
        String::from_utf8_lossy(&dropped.stderr).contains("does not keep HEAD's tree"),
        "{report}"
    );
    assert_eq!(still, head, "a plan that changes the tree moved the branch");

    assert!(rewritten.status.success(), "{report}");
    let outcome: Value = serde_json::from_slice(&rewritten.stdout).expect("the outcome is JSON");
    let new_head = git(&clone, &["rev-parse", "HEAD"]);
    assert_eq!(outcome["to"], json!(new_head), "{report}");
    assert_eq!(
        git(&clone, &["rev-parse", "HEAD^{tree}"]),
        tree,
        "the tree changed"
    );
    let subjects = git(&clone, &["log", "--format=%s", &format!("{pushed}..HEAD")]);
    assert_eq!(
        subjects.lines().collect::<Vec<_>>(),
        [
            "Carries a note",
            "Add right",
            "Add left",
            "Add the named file",
            "Add the feature"
        ],
        "{report}"
    );
    assert_eq!(
        git(&clone, &["status", "--porcelain"]),
        "",
        "the working tree changed"
    );
    assert_eq!(
        git(&clone, &["notes", "--ref", NOTES, "show", "HEAD"]),
        "attested"
    );
    let annotated: BTreeSet<String> = git(&clone, &["notes", "--ref", NOTES, "list"])
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .map(str::to_string)
        .collect();
    assert_eq!(annotated, BTreeSet::from([noted, new_head]), "{report}");
    assert_eq!(
        git(&upstream, &["rev-parse", "main"]),
        pushed,
        "the upstream moved"
    );
}
