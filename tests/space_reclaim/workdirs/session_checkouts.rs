//! The janitor takes the git checkouts a fleet session is proven to have
//! cloned, and nothing else: below the home on every pass, below the work
//! root at the disk-full rule. A checkout below
//! `~/Documents/CodingProjects` (both the older `<repository>` and the later
//! `Wisent/<repository>` layout) or below the declared work root's
//! `session-checkouts` goes once its clone marker names the session that
//! cloned it, nothing in it exists only there and no process works in it.
//! It keeps, and names, a checkout holding a commit no remote has (one only
//! a local tag reaches included) or an uncommitted change; keeps one a
//! running process works in; keeps the caller's own clean, fully pushed
//! checkout no session cloned on a host that declares no ownership and
//! grants the janitor its Documents folder; never touches a directory that
//! is no checkout; and on a shared host keeps every checkout under the home.
//! Real git, a real process and a real APFS volume under the repository's
//! build directory:
//! https://stado.wisent.com/docs/capabilities/space#real-tests

#[path = "session_keeps.rs"]
mod keeps;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use stado::providers::local::work_base::{SESSION_CHECKOUTS_LEAF, SESSION_CLONE_MARKER};

use crate::native::Native;

/// The janitor's report section and removal-record name for this cleaner.
const CLEANER: &str = "session_checkouts";

/// Git run by the test itself, with none of this machine account's
/// configuration, so no hook or setting of the person running it reaches
/// the fixtures.
pub(super) fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=Stado test"])
        .args(["-c", "user.email=stado-test@example.invalid"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} in {} exited {:?}: {}",
        directory.display(),
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A bare repository holding one commit: the remote every checkout clones.
pub(super) fn origin(native: &Native) -> PathBuf {
    let seed = native.home.join("fixtures/seed");
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "--quiet"]);
    fs::write(seed.join("README"), b"session checkout fixture\n").unwrap();
    git(&seed, &["add", "README"]);
    git(&seed, &["commit", "--quiet", "-m", "seed"]);
    let bare = native.home.join("fixtures/origin.git");
    let (seed, bare_path) = (seed.to_str().unwrap(), bare.to_str().unwrap());
    git(&native.home, &["clone", "--quiet", "--bare", seed, bare_path]);
    bare
}

/// A checkout of `origin` at `checkout`, everything in it on the remote,
/// that no session cloned: the person's own.
pub(super) fn clone(origin: &Path, checkout: &Path) {
    let parent = checkout.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    let (origin, path) = (origin.to_str().unwrap(), checkout.to_str().unwrap());
    git(parent, &["clone", "--quiet", origin, path]);
}

/// A checkout of `origin` at `checkout` as an `omp-session` clones one: the
/// clone, then the clone marker in its `.git` naming the session's job.
pub(super) fn session_clone(origin: &Path, checkout: &Path) {
    clone(origin, checkout);
    let marker = checkout.join(".git").join(SESSION_CLONE_MARKER);
    fs::write(marker, b"session-checkouts-journey-job\n").unwrap();
}

/// Declare in the isolated registry what a fleet host declares for its
/// janitor: this program may open `~/Documents`, where the checkouts are.
pub(super) fn admit_documents(native: &Native) {
    let grants = json!([{
        "program": env!("CARGO_BIN_EXE_stado"),
        "folder": "documents",
        "reason": "the janitor takes the checkouts fleet sessions cloned under ~/Documents",
    }]);
    let grants = grants.to_string();
    let path = "targets.0.privacy_grants";
    native.success(&["registry", "set", "--path", path, "--value", &grants, "--json"]);
}

/// One enforcing janitor pass on this machine with `work_root` declared, as
/// the agent declares it to every pass it runs.
pub(super) fn cleanup_with_work_root(native: &Native, work_root: &Path) -> Value {
    let args = [
        "space",
        "reclaim",
        "--local",
        "--apply",
        "--reason",
        "session checkouts journey",
    ];
    let output = native
        .command(&args)
        .env("STADO_WORK_ROOT", work_root)
        .output()
        .unwrap();
    native.record(env!("CARGO_BIN_EXE_stado"), &args, &output);
    assert!(
        output.status.success(),
        "stado {args:?} exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    native.observe("cleanup", report.clone());
    report
}

pub(super) fn errors_naming(report: &Value, needle: &str) -> bool {
    report["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|error| error.as_str().is_some_and(|text| text.contains(needle)))
}

/// Whether anything moved aside for removal is left in `directory`.
fn aside_left(directory: &Path) -> bool {
    fs::read_dir(directory).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".stado-removing"))
    })
}

#[test]
fn proven_session_checkouts_go_at_the_rule_and_every_other_one_stays() {
    let native = Native::new("session-checkouts");
    admit_documents(&native);
    let origin = origin(&native);
    let projects = native.home.join("Documents/CodingProjects");
    let work_root = native.home.join("fleet-work");
    let sessions = work_root.join(SESSION_CHECKOUTS_LEAF);
    let clean = projects.join("Wisent/clean-repository");
    let older = projects.join("older-layout-repository");
    let on_work_root = sessions.join("work-root-repository");
    let unpushed = projects.join("Wisent/unpushed-repository");
    let uncommitted = projects.join("Wisent/uncommitted-repository");
    let worked_in = projects.join("Wisent/worked-in-repository");
    let tagged = sessions.join("tagged-repository");
    for checkout in [
        &clean,
        &older,
        &on_work_root,
        &unpushed,
        &uncommitted,
        &worked_in,
        &tagged,
    ] {
        session_clone(&origin, checkout);
    }
    // The person's own clean, fully pushed checkout, on a host that declares
    // no ownership and grants the janitor its Documents folder.
    let own = projects.join("Wisent/own-repository");
    clone(&origin, &own);
    fs::write(unpushed.join("NOTES"), b"work no remote has\n").unwrap();
    git(&unpushed, &["add", "NOTES"]);
    git(&unpushed, &["commit", "--quiet", "-m", "unpushed"]);
    let unpushed_commit = git(&unpushed, &["rev-parse", "HEAD"]);
    fs::write(uncommitted.join("DRAFT"), b"not committed\n").unwrap();
    // A commit only a local tag reaches: no branch holds it.
    fs::write(tagged.join("HELD"), b"work only a tag reaches\n").unwrap();
    git(&tagged, &["add", "HELD"]);
    git(&tagged, &["commit", "--quiet", "-m", "held by a tag"]);
    let tagged_commit = git(&tagged, &["rev-parse", "HEAD"]);
    git(&tagged, &["tag", "held-by-a-tag"]);
    git(&tagged, &["reset", "--quiet", "--hard", "@{upstream}"]);
    let notes = projects.join("Wisent/notes/plan.txt");
    fs::create_dir_all(notes.parent().unwrap()).unwrap();
    fs::write(&notes, b"not a checkout\n").unwrap();
    // A session's harness works in its checkout for as long as it runs.
    let mut session = Command::new("/bin/sleep")
        .arg("600")
        .current_dir(&worked_in)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();

    let user_data = native.fill_with_user_data();
    let report = cleanup_with_work_root(&native, &work_root);
    session.kill().unwrap();
    session.wait().unwrap();
    let cleaner = &report["cleaners"][CLEANER];
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    for taken in [&clean, &older, &on_work_root] {
        assert!(!taken.exists(), "{} was kept: {report}", taken.display());
    }
    assert_eq!(cleaner["deleted_items"], 3, "{report}");
    for kept in [&own, &unpushed, &uncommitted, &worked_in, &tagged] {
        assert!(kept.join(".git").is_dir(), "{} was taken: {report}", kept.display());
    }
    assert_eq!(cleaner["skipped"]["not_session_checkout"], 1, "{report}");
    assert_eq!(cleaner["skipped"]["unpushed_work"], 3, "{report}");
    assert_eq!(cleaner["skipped"]["held_by_process"], 1, "{report}");
    for named in [unpushed_commit.as_str(), tagged_commit.as_str(), "DRAFT"] {
        assert!(errors_naming(&report, named), "{named} is not named: {report}");
    }
    for root in [projects.join("Wisent"), projects.clone(), sessions.clone()] {
        assert!(!aside_left(&root), "a removal left its aside in {}", root.display());
    }
    assert_eq!(fs::read(&notes).unwrap(), b"not a checkout\n");
    assert!(user_data.is_file(), "the user's data was deleted");

    let after = native.json(&["space", "candidates", "--json"]);
    assert!(
        after["removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line["cleaner"] == CLEANER && line["path"].as_str() == clean.to_str()),
        "the removal is not recorded: {after}"
    );
}

#[test]
fn on_a_shared_host_checkouts_under_the_home_stay_and_the_work_roots_proven_ones_go() {
    let native = Native::new("session-checkouts-shared");
    native.success(&[
        "registry",
        "set",
        "--path",
        "targets.0.ownership",
        "--value",
        "shared",
        "--json",
    ]);
    admit_documents(&native);
    let origin = origin(&native);
    let work_root = native.home.join("fleet-work");
    let hers = native
        .home
        .join("Documents/CodingProjects/Wisent/her-repository");
    let fleet = work_root.join(SESSION_CHECKOUTS_LEAF).join("fleet-repository");
    let unproven = work_root.join(SESSION_CHECKOUTS_LEAF).join("unproven-repository");
    session_clone(&origin, &hers);
    session_clone(&origin, &fleet);
    clone(&origin, &unproven);

    native.fill_with_user_data();
    let report = cleanup_with_work_root(&native, &work_root);
    let cleaner = &report["cleaners"][CLEANER];
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert!(hers.join(".git").is_dir(), "a checkout in her home was taken: {report}");
    assert_eq!(cleaner["skipped"]["shared_host_not_stado_owned"], 1, "{report}");
    assert!(!fleet.exists(), "the work root's proven checkout was kept: {report}");
    assert!(unproven.join(".git").is_dir(), "an unproven checkout was taken: {report}");
    assert_eq!(cleaner["skipped"]["not_session_checkout"], 1, "{report}");
    assert_eq!(cleaner["deleted_items"], 1, "{report}");
}

/// A host whose home volume is full and whose declared work root is a
/// roomy data disk is not at the rule: the rule judges the volume the fleet
/// writes to. A proven session checkout earlier releases left under the home
/// goes anyway, since nothing reuses it, and the work root's stays for the
/// next session.
#[test]
fn a_full_home_volume_is_not_the_rule_and_its_legacy_checkouts_still_go() {
    let native = Native::new("session-checkouts-fleet-volume");
    admit_documents(&native);
    let origin = origin(&native);
    let data = native.data_volume("fleet-work");
    let legacy = native
        .home
        .join("Documents/CodingProjects/Wisent/legacy-repository");
    let reused = data.path().join("session-checkouts/reused-repository");
    session_clone(&origin, &legacy);
    session_clone(&origin, &reused);

    let user_data = native.fill_with_user_data();
    let report = cleanup_with_work_root(&native, data.path());
    let cleaner = &report["cleaners"][CLEANER];
    assert_eq!(report["rule"]["triggered"], false, "{report}");
    assert!(!legacy.exists(), "the legacy checkout was kept: {report}");
    assert!(
        reused.join(".git").is_dir(),
        "the work root's checkout went below the rule: {report}"
    );
    assert_eq!(cleaner["deleted_items"], 1, "{report}");
    assert!(user_data.is_file(), "the user's data was deleted");
}
