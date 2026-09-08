//! The refusals that matter, each with the sentence it is contracted to say.
//!
//! The sentences and exit codes below were copied from live runs of the built
//! binary against this same fixture, not written from the source: an unknown
//! host and an undeclared cleanup scope leave through `CmdError::click` and
//! exit 1, while an undeclared stage and a missing reason are usage refusals
//! and exit 2. Each case also reads the filesystem afterwards, because a
//! refusal that already deleted something is not a refusal.

use std::fs;

use crate::fixture::{Host, BUILD_WORK_ROOT, JANITOR_STATE, TARGET, UNDECLARED_TARGET};
use crate::system::said;

/// `CmdError::click` — a refusal about the fleet's own state.
const CLICK_EXIT: i32 = 1;
/// `CmdError::usage` — a refusal about the command line itself.
const USAGE_EXIT: i32 = 2;
/// A payload small enough to be quick and large enough to be visible.
const SCRATCH_MIB: usize = 1;

fn refused(output: &std::process::Output, code: i32, sentence: &str) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "expected exit {code}, got {:?}\nstderr:\n{}",
        output.status.code(),
        said(&output.stderr)
    );
    let stderr = said(&output.stderr);
    assert!(
        stderr.contains(sentence),
        "the refusal did not say {sentence:?}:\n{stderr}"
    );
}

/// A host nobody declared is refused by the read and by the reclamation, both
/// naming the target they could not find.
#[test]
fn an_unknown_host_is_refused_by_the_read_and_the_reclamation() {
    let host = Host::new();
    let sentence = "target 'no-such-host' is not in the canonical registry";

    refused(
        &host.run(&["space", "report", "no-such-host", "--json"]),
        CLICK_EXIT,
        sentence,
    );
    refused(
        &host.run(&[
            "space",
            "reclaim",
            "no-such-host",
            "--stage",
            "build_scratch",
            "--dry-run",
        ]),
        CLICK_EXIT,
        sentence,
    );
    assert!(
        !host.under_home(JANITOR_STATE).exists(),
        "a refused reclamation still ran the janitor"
    );
}

/// An undeclared stage and a missing reason are both refused before the host
/// is touched, and the scope they would have swept is still there.
#[test]
fn an_undeclared_stage_and_a_missing_reason_stop_before_the_host() {
    let host = Host::new();
    let scratch_root = host.under_home(BUILD_WORK_ROOT);
    fs::create_dir_all(&scratch_root).expect("create the build scratch root");
    let tree = host.seed_tree(&scratch_root, "release-tree", SCRATCH_MIB, false);

    refused(
        &host.run(&[
            "space",
            "reclaim",
            TARGET,
            "--stage",
            "mystery",
            "--dry-run",
        ]),
        USAGE_EXIT,
        "stage 'mystery' is not declared; add it to stado-rs/data/space.json reclaim_stages",
    );
    refused(
        &host.run(&[
            "space",
            "reclaim",
            TARGET,
            "--stage",
            "build_scratch",
            "--apply",
        ]),
        USAGE_EXIT,
        "space reclaim --apply removes files and needs --reason <text>; the reason is appended \
         to the target's own audit log beside the state it changed. Run without --apply to \
         preview the declared stages",
    );

    assert!(
        tree.join("payload.bin").is_file(),
        "a refused reclamation removed the scope anyway"
    );
    assert!(
        !host.under_home(JANITOR_STATE).exists(),
        "a refused reclamation still ran the janitor"
    );
}

/// A host that declares no cleanup scope, no cache cleaner, or a root outside
/// an absolute or home-relative path is refused with its own sentence, and the
/// registry it was refused over is left exactly as it was.
#[test]
fn a_scope_nobody_declared_is_refused_with_its_own_sentence() {
    let host = Host::new();
    let registry = host.storage.join("registry.json");

    host.declare_no_scope();
    let declared = fs::read_to_string(&registry).expect("read the fixture registry");
    refused(
        &host.run(&["space", "report", UNDECLARED_TARGET, "--json"]),
        CLICK_EXIT,
        &format!(
            "{UNDECLARED_TARGET} declares no disk cleanup policy; add it to registry \
             targets[].disk_cleanup"
        ),
    );
    assert_eq!(
        fs::read_to_string(&registry).expect("read the fixture registry"),
        declared,
        "a refusal rewrote the registry"
    );

    host.declare(&policy(r#"{}"#));
    refused(
        &host.run(&["space", "report", TARGET, "--json"]),
        CLICK_EXIT,
        &format!(
            "{TARGET} declares no build cache cleaner; add it to registry \
             targets[].disk_cleanup.cleaners.build_caches"
        ),
    );

    host.declare(&policy(
        r#"{"build_caches": {"min_age_seconds": 86400, "root": "relative/cache"}}"#,
    ));
    refused(
        &host.run(&["space", "report", TARGET, "--json"]),
        CLICK_EXIT,
        &format!(
            "{TARGET} declares build cache root \"relative/cache\" outside an absolute or \
             home-relative path; fix registry targets[].disk_cleanup.cleaners.build_caches.root"
        ),
    );
}

/// A reporting policy whose cleaner table is `cleaners`. The watermark and
/// budget values are registry configuration for the fixture and are never
/// reached here: every case using this policy is refused before a pass runs.
fn policy(cleaners: &str) -> String {
    format!(
        r#"{{
        "mode": "report",
        "check_interval_seconds": 3600,
        "low_free_gb": 1,
        "target_free_gb": 2,
        "max_bytes_per_pass": 1073741824,
        "max_items_per_pass": 32,
        "max_scan_items": 4096,
        "cleaners": {cleaners}
      }}"#
    )
}
