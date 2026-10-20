//! A line that merged its upstream is rewritten onto the upstream: the
//! clone adds a feature and a commit that only reformats it, takes a commit
//! the upstream gained meanwhile through a merge, and adds one more commit.
//! The plan names the three single-parent commits and folds the format
//! commit into the feature; the real binary replays them onto the upstream
//! without the merge, the tree stays the old HEAD's, and the new line holds
//! no merge. A merge of a second line the upstream does not hold is refused
//! by name and moves nothing.

use serde_json::json;

use super::fixture::{commit, fixture, git, Fixture, Journey};

/// A merge commit of HEAD and `other` holding `tree`, made with plumbing.
fn merge(root: &std::path::Path, other: &str, tree: &str, message: &str) -> String {
    let head = git(root, &["rev-parse", "HEAD"]);
    let made = git(
        root,
        &["commit-tree", tree, "-p", &head, "-p", other, "-m", message],
    );
    git(root, &["update-ref", "HEAD", &made]);
    git(root, &["read-tree", "--reset", "-u", "HEAD"]);
    made
}

#[test]
fn a_line_that_merged_its_upstream_is_replayed_onto_it_and_another_merge_is_refused() {
    let Fixture {
        checkout,
        area,
        upstream,
        clone,
    } = fixture();
    let base = git(&clone, &["rev-parse", "HEAD"]);
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

    // The upstream gains a commit of its own on the base.
    git(&clone, &["checkout", "--quiet", "--detach", &base]);
    let gained = commit(&clone, &[("upstream.txt", "upstream\n")], "Upstream's own");
    git(&clone, &["checkout", "--quiet", "main"]);
    git(
        &upstream,
        &[
            "fetch",
            "--quiet",
            clone.to_str().expect("path"),
            &format!("{gained}:main"),
        ],
    );
    git(&clone, &["fetch", "--quiet", "origin"]);
    let pushed = git(&upstream, &["rev-parse", "main"]);
    let joined = git(
        &clone,
        &["merge-tree", "--write-tree", "HEAD", "origin/main"],
    );
    merge(&clone, &pushed, &joined, "Take the upstream's commit");
    let after = commit(&clone, &[("after.txt", "after\n")], "Add after the merge");
    let head = git(&clone, &["rev-parse", "HEAD"]);
    let tree = git(&clone, &["rev-parse", "HEAD^{tree}"]);
    let mut journey = Journey {
        area: area.clone(),
        clone: clone.clone(),
        log: Vec::new(),
    };

    let replayed = journey.history(&json!({"commits": [
        {"action": "pick", "commit": feature},
        {"action": "fold", "commit": format, "into": feature},
        {"action": "pick", "commit": after},
    ]}));
    let new_head = git(&clone, &["rev-parse", "HEAD"]);

    // A second unpushed line merged in is not the upstream's.
    git(&clone, &["checkout", "--quiet", "--detach", &new_head]);
    let side = commit(&clone, &[("side.txt", "side\n")], "A side line");
    git(&clone, &["checkout", "--quiet", "main"]);
    let side_tree = git(&clone, &["merge-tree", "--write-tree", "HEAD", &side]);
    let side_merge = merge(&clone, &side, &side_tree, "Take the side line");
    let refused = journey.history(&json!({"commits": [
        {"action": "pick", "commit": git(&clone, &["rev-parse", "HEAD~1"])},
    ]}));
    let still = git(&clone, &["rev-parse", "HEAD"]);
    let report = journey.report(&checkout);

    assert!(replayed.status.success(), "{report}");
    assert_ne!(new_head, head, "{report}");
    assert_eq!(
        git(&clone, &["rev-parse", &format!("{new_head}^{{tree}}")]),
        tree,
        "the replayed line does not keep HEAD's tree: {report}"
    );
    assert_eq!(
        git(
            &clone,
            &[
                "rev-list",
                "--min-parents=2",
                &format!("{pushed}..{new_head}")
            ]
        ),
        "",
        "the replayed line still holds a merge: {report}"
    );
    assert_eq!(
        git(
            &clone,
            &["log", "--format=%s", &format!("{pushed}..{new_head}")]
        )
        .lines()
        .collect::<Vec<_>>(),
        ["Add after the merge", "Add the feature"],
        "{report}"
    );
    assert!(!refused.status.success(), "{report}");
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains(&format!("the merge {side_merge} of {side}")),
        "the refusal does not name the merge and its other line: {report}"
    );
    assert_eq!(still, side_merge, "a refused plan moved the branch");
}
