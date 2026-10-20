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

use serde_json::{json, Value};

mod fixture;
mod merged;

use fixture::{commit, fixture, git, Fixture, Journey, NOTES};

#[test]
fn a_plan_folds_rewords_splits_and_keeps_notes_and_a_bad_plan_moves_nothing() {
    let Fixture {
        checkout,
        area,
        upstream,
        clone,
    } = fixture();
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
    let report = journey.report(&checkout);

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
