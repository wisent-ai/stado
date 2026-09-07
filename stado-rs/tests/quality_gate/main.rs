//! `tests/release_quality_gate.sh` says whose revision a refused quality step
//! belongs to.
//!
//! Every test runs the real script with `bash`, against a real git repository
//! it builds in a temp dir: a base commit, a head commit, and a manifest whose
//! quality step is a real command with a deterministic verdict. Nothing is
//! stubbed — the script resolves the platform, reads the manifest with `jq`,
//! executes the declared argv, and on a refusal checks out the base in its own
//! worktree and runs the same argv there.
//!
//! # The incident
//!
//! A pull request is judged on its merge result, so a quality step that
//! already refuses `main` refuses every pull request opened against it. On
//! 2026-09-04 and 2026-09-05 `main` carried unformatted `cli/onboarding.rs`,
//! then `cli/identity.rs`, then `dashboard/mod.rs`; each time the gate told
//! the author of an unrelated change to "fix the tree", three authors
//! reformatted somebody else's code to get their own work through, and the
//! revision that introduced it was never named.
//!
//! What is defended: a clean tree passes; a failure the head introduces is
//! reported as `verdict=introduced`; a failure the base already carries is
//! reported as `verdict=inherited` with the base sha; and a base that cannot
//! be resolved is `verdict=unattributed` rather than either verdict.

use std::path::Path;
use std::process::{Command, Output};

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "gate test")
        .env("GIT_AUTHOR_EMAIL", "gate@example.invalid")
        .env("GIT_COMMITTER_NAME", "gate test")
        .env("GIT_COMMITTER_EMAIL", "gate@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A manifest whose one quality step asserts that `marker` holds `OK`, using
/// programs that exist on every runner this repository uses.
fn manifest(platform: &str) -> String {
    serde_json::json!({
        "platforms": {
            platform: {
                "quality": [{
                    "name": "marker",
                    "argv": ["/usr/bin/grep", "-q", "OK", "marker"],
                }],
            },
        },
    })
    .to_string()
}

fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// A repository whose base commit holds `base_marker` and whose checked-out
/// head commit holds `head_marker`.
///
/// Head is always a commit of its own, even when it carries the same marker:
/// a base that IS the head is a comparison against itself, which the script
/// refuses as evidence and which `a_base_that_is_this_revision_*` covers.
fn repository(base_marker: &str, head_marker: &str) -> tempfile::TempDir {
    let repo = tempfile::tempdir().expect("temp repo");
    let root = repo.path();
    git(root, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(root.join(".wisent-release.json"), manifest(platform())).unwrap();
    std::fs::write(root.join("marker"), format!("{base_marker}\n")).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "--quiet", "-m", "base"]);
    // `origin/main` is what the script defaults to, and a self-remote is how a
    // temp repository can have one without a network.
    git(root, &["remote", "add", "origin", "."]);
    git(root, &["fetch", "--quiet", "origin", "main"]);
    git(root, &["update-ref", "refs/remotes/origin/main", "main"]);
    std::fs::write(root.join("marker"), format!("{head_marker}\n")).unwrap();
    std::fs::write(root.join("head-note"), "a change of its own\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "--quiet", "-m", "head"]);
    repo
}

fn run_gate(repo: &Path, base: Option<&str>) -> Output {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/release_quality_gate.sh");
    let mut command = Command::new("bash");
    command.arg(&script).current_dir(repo);
    if let Some(base) = base {
        command.args(["--base", base]);
    }
    command.output().expect("release quality gate runs")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_clean_tree_passes_and_names_the_steps_it_ran() {
    let repo = repository("OK", "OK");
    let out = run_gate(repo.path(), None);
    assert!(out.status.success(), "{}", stderr(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("declares 1 quality step(s)") && stdout.contains("passed."),
        "{stdout}"
    );
}

#[test]
fn a_failure_this_revision_introduces_is_attributed_to_it() {
    let repo = repository("OK", "BAD");
    let out = run_gate(repo.path(), None);
    assert!(!out.status.success(), "a refused step must fail the gate");
    let stderr = stderr(&out);
    assert!(
        stderr.contains("verdict=introduced") && stderr.contains("this change introduces it"),
        "{stderr}"
    );
}

#[test]
fn a_failure_the_base_already_carries_is_not_the_authors() {
    let repo = repository("BAD", "BAD");
    let out = run_gate(repo.path(), None);
    assert!(!out.status.success(), "an inherited failure still fails");
    let stderr = stderr(&out);
    assert!(
        stderr.contains("verdict=inherited"),
        "an inherited failure must not be reported as introduced: {stderr}"
    );
    assert!(
        stderr.contains("did not introduce it"),
        "the sentence has to say so in words: {stderr}"
    );
    // The base sha is in the message, so the operator can read who to ask.
    let base = Command::new("git")
        .args(["rev-parse", "refs/remotes/origin/main"])
        .current_dir(repo.path())
        .output()
        .expect("git rev-parse");
    let sha = String::from_utf8_lossy(&base.stdout).trim().to_string();
    assert!(stderr.contains(&sha), "expected base {sha} in: {stderr}");
}

#[test]
fn a_base_that_cannot_be_resolved_is_unattributed_not_guessed() {
    let repo = repository("OK", "BAD");
    let out = run_gate(repo.path(), Some("refs/heads/no-such-base"));
    assert!(!out.status.success());
    let stderr = stderr(&out);
    assert!(
        stderr.contains("verdict=unattributed") && stderr.contains("could not be checked out"),
        "an unresolvable base is stated, never inferred: {stderr}"
    );
}

#[test]
fn a_base_that_is_this_revision_measures_nothing() {
    // What a push to `main` looks like when the base is read as `origin/main`:
    // the revision under judgement compared against itself, which would answer
    // `inherited` for every failure and never name the push that caused it.
    let repo = repository("OK", "BAD");
    let out = run_gate(repo.path(), Some("HEAD"));
    assert!(!out.status.success());
    let stderr = stderr(&out);
    assert!(
        stderr.contains("verdict=unattributed") && stderr.contains("IS this revision"),
        "a self-comparison is refused as evidence, not read as inherited: {stderr}"
    );
}

#[test]
fn an_uncommitted_break_at_the_base_commit_is_still_attributed() {
    // How this runs by hand: a checkout of `main` with the break not yet
    // committed. The base sha equals `HEAD`, but the base's own worktree does
    // not carry the break, so the comparison is real and the answer is the
    // developer's own change.
    let repo = repository("OK", "OK");
    std::fs::write(repo.path().join("marker"), "BAD\n").unwrap();
    let out = run_gate(repo.path(), Some("HEAD"));
    assert!(!out.status.success());
    let stderr = stderr(&out);
    assert!(
        stderr.contains("verdict=introduced"),
        "a dirty tree at the base commit is attributable: {stderr}"
    );
}

#[test]
fn a_green_run_still_names_the_revision_it_compared_against() {
    // A check whose comparison basis is invisible while it passes cannot be
    // audited, and the push path reads its base from a different field than
    // the pull-request path does.
    let repo = repository("OK", "OK");
    let out = run_gate(repo.path(), None);
    assert!(out.status.success(), "{}", stderr(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let base = Command::new("git")
        .args(["rev-parse", "refs/remotes/origin/main"])
        .current_dir(repo.path())
        .output()
        .expect("git rev-parse");
    let sha = String::from_utf8_lossy(&base.stdout).trim().to_string();
    assert!(
        stdout.contains(&format!("base: {sha} (origin/main)")),
        "expected the resolved base in: {stdout}"
    );

    // And when there is none, the line says which reason.
    let out = run_gate(repo.path(), Some("HEAD"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("base: none -- HEAD is this revision"),
        "expected the stated absence in: {stdout}"
    );
}

#[test]
fn an_empty_quality_declaration_is_refused_rather_than_passed() {
    let repo = repository("OK", "OK");
    std::fs::write(
        repo.path().join(".wisent-release.json"),
        serde_json::json!({"platforms": {platform(): {"quality": []}}}).to_string(),
    )
    .unwrap();
    let out = run_gate(repo.path(), None);
    assert!(!out.status.success(), "an empty gate is a refusal");
    assert!(stderr(&out).contains("would be a lie"), "{}", stderr(&out));
}
