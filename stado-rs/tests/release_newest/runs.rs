//! What a run already on record says about the version a checkout declares.
//!
//! On 2026-09-23 stado 0.21.54's run failed its darwin quality gate, the fix
//! was committed at the same version, and `newest` answered "already
//! published" because a run existed at all: the fix could not be released.
//! Only a run that published settles a version, and only a run still moving
//! on the very commit the checkout stands on is waited for.

use crate::area::{releasing_manifest, Area};
use crate::{document, entry};

/// One checkout, the run on record for its version, and what the plan must
/// say about it.
struct Case {
    product: &'static str,
    run_state: &'static str,
    run_is_of_this_commit: bool,
    standing: &'static str,
}

const CASES: [Case; 5] = [
    Case {
        product: "failed-run",
        run_state: "failed",
        run_is_of_this_commit: true,
        standing: "releasable",
    },
    Case {
        product: "superseded-run",
        run_state: "superseded",
        run_is_of_this_commit: true,
        standing: "releasable",
    },
    Case {
        product: "older-commit-run",
        run_state: "waiting",
        run_is_of_this_commit: false,
        standing: "releasable",
    },
    Case {
        product: "moving-run",
        run_state: "waiting",
        run_is_of_this_commit: true,
        standing: "in_flight",
    },
    Case {
        product: "published-run",
        run_state: "completed",
        run_is_of_this_commit: false,
        standing: "published",
    },
];

/// Every run state that leaves the commit to release, and every one that
/// does not, read by the real binary from the area's own store.
#[test]
fn only_a_published_run_or_one_still_moving_on_this_commit_holds_a_version_back() {
    let area = Area::new("runs");
    for case in &CASES {
        let checkout = area.checkout(
            case.product,
            &releasing_manifest(case.product),
            Some(("package.json", "{\"version\": \"0.21.54\"}")),
        );
        let head = area.head(&checkout);
        let commit = if case.run_is_of_this_commit {
            head.clone()
        } else {
            "0".repeat(head.len())
        };
        let run_id = format!("run-{}", case.product);
        area.record_run(&run_id, case.product, "0.21.54", case.run_state, &commit);
    }

    let planned = area.plan(&[]);
    assert!(
        planned.status.success(),
        "the plan failed: {}",
        String::from_utf8_lossy(&planned.stderr)
    );
    let report = document(&planned);
    for case in &CASES {
        let found = entry(&report, case.product);
        assert_eq!(
            found["standing"],
            case.standing,
            "a {} run {} this commit: {found}",
            case.run_state,
            if case.run_is_of_this_commit {
                "of"
            } else {
                "older than"
            }
        );
        if case.standing != "releasable" {
            assert_eq!(
                found["run"],
                format!("run-{}", case.product),
                "the run that holds the version back is named: {found}"
            );
        }
    }

    let listing = area.stado(&[
        "release",
        "newest",
        "--root",
        area.workspace.to_str().expect("a UTF-8 workspace"),
        "--plan",
    ]);
    let text = String::from_utf8_lossy(&listing.stdout);
    assert!(
        text.contains("is being released by run run-moving-run"),
        "the listing names the run still moving: {text}"
    );
    assert!(
        text.contains("3 of 5 product(s) would be released"),
        "the listing counts what would be released: {text}"
    );
}
