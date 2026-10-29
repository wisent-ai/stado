//! A CLI command tree no model could judge does not decide the check: the
//! review is reported as not run, with its cause, and the gates decide.
//!
//! Oko is left off `PATH`, so the audit's model reading obtains no judgment.
//! A commit carrying HEAD's tree is made in the Stado checkout, reachable
//! from no ref, with a message that attests nothing. The real `stado quality
//! check` passes it, saying the command-tree review of that revision was not
//! run and why. Revisions, invocations, statuses and both streams are
//! retained under `.build/quality-command-tree/`. Needs the release build
//! toolchain.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

/// `PATH` without every directory that holds an `oko`, so Oko's model call
/// cannot run.
fn path_without_oko() -> String {
    let path = std::env::var_os("PATH").expect("PATH is set");
    let kept: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|directory| !directory.join("oko").exists())
        .collect();
    std::env::join_paths(kept)
        .expect("PATH joins")
        .into_string()
        .expect("PATH is UTF-8")
}

/// A commit with HEAD's tree and HEAD as parent whose message is `message`,
/// made in `root` without moving any ref.
fn commit(root: &Path, area: &Path, name: &str, message: &str) -> String {
    let file = area.join(format!("{name}.message"));
    fs::write(&file, message).expect("write the fixture commit's message");
    let tree = super::git(root, &["rev-parse", "HEAD^{tree}"]);
    super::git(
        root,
        &[
            "commit-tree",
            &tree,
            "-p",
            "HEAD",
            "-F",
            file.to_str().expect("message path"),
        ],
    )
}

fn quality(root: &Path, home: &Path, path: &str, revision: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .env("HOME", home)
        .env("PATH", path)
        .args([
            "quality",
            "check",
            "--root",
            root.to_str().expect("repository path"),
            "--revision",
            revision,
        ])
        .output()
        .expect("run real committed-source quality check")
}

#[test]
fn unjudged_tree_is_reported_as_not_reviewed_and_the_gates_decide() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("Stado checkout")
        .to_path_buf();
    let home = super::isolated_home(&root);
    fs::create_dir_all(home.join(".stado/bin")).expect("isolated quality run area");
    let path = path_without_oko();

    let revision = commit(
        &root,
        &home,
        "unattested",
        "Fixture commit with HEAD's tree and no attestation\n",
    );
    let passed = quality(&root, &home, &path, &revision);
    let report = json!({
        "source_revision": super::git(&root, &["rev-parse", "HEAD"]),
        "binary": env!("CARGO_BIN_EXE_stado"),
        "path": path,
        "commands": [
            {"argv": ["stado", "quality", "check", "--root", root, "--revision", revision],
             "exit_status": passed.status.code(), "stdout": String::from_utf8_lossy(&passed.stdout),
             "stderr": String::from_utf8_lossy(&passed.stderr)}
        ]
    });
    let report_path = home.join("report.json");
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("serialize report"),
    )
    .expect("retain report");
    eprintln!(
        "unjudged CLI command-tree evidence: {}",
        report_path.display()
    );

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&passed.stdout),
        String::from_utf8_lossy(&passed.stderr)
    );
    assert!(
        passed.status.success(),
        "an unjudged tree failed gates that passed: {report}"
    );
    assert!(
        said.contains(&format!(
            "CLI command-tree review of {revision} was not run: CLI command-tree judgment through Brama failed"
        )) && said.contains("the gates above decided this check"),
        "the pass does not say the review was not run and why: {report}"
    );
}
