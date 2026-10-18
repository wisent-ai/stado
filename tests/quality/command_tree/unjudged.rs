//! A CLI command tree no model could judge stands on the checked commit's
//! attestation, and only on one that stands.
//!
//! Oko is left off `PATH`, so the audit's model reading obtains no judgment.
//! Two commits carrying HEAD's tree are made in the Stado checkout, reachable
//! from no ref: one whose message carries every claim and trailer the
//! installed Tama's commit standard requires, one whose message carries none.
//! The real `stado quality check` passes the first, saying the tree was not
//! judged and stands on its attestation, and refuses the second, naming the
//! judgment's failure and that the attestation does not stand. Revisions,
//! invocations, statuses and both streams are retained under
//! `.build/quality-command-tree/`. Needs the release build toolchain and an
//! installed Tama at `~/.stado/bin/tama`; a missing one fails the journey.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

/// The installed Tama the product reads attestations with, from the
/// operator's home.
fn installed_tama() -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME names the operator's home");
    let tama = Path::new(&home).join(".stado/bin/tama");
    assert!(
        tama.is_file(),
        "an unjudged command tree is read by the installed Tama at {}",
        tama.display()
    );
    tama
}

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

/// The message of a commit attested as the installed Tama's commit standard
/// asks: every claim, then one trailer of each kind it requires.
fn attested_message(tama: &Path) -> String {
    let output = Command::new(tama)
        .args(["attestation", "standard", "--commit", "--json"])
        .output()
        .expect("run tama attestation standard");
    assert!(output.status.success(), "{output:?}");
    let standard: Value = serde_json::from_slice(&output.stdout).expect("the standard is JSON");
    let claims: Vec<&str> = standard["claims"]
        .as_array()
        .expect("claims")
        .iter()
        .map(|claim| claim.as_str().expect("each claim is text"))
        .collect();
    let trailers: Vec<String> = standard["trailers"]["required"]
        .as_array()
        .expect("required trailers")
        .iter()
        .map(|required| {
            format!(
                "{}: the fixture carries HEAD's tree, so its patch is empty",
                required["trailer"].as_str().expect("trailer name")
            )
        })
        .collect();
    format!(
        "Fixture commit with HEAD's tree\n\n{}\n\n{}\n",
        claims.join("\n"),
        trailers.join("\n")
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
fn unjudged_tree_stands_only_on_an_attestation_that_stands() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("Stado checkout")
        .to_path_buf();
    let home = root
        .join(".build/quality-command-tree")
        .join(uuid::Uuid::new_v4().to_string());
    let bin = home.join(".stado/bin");
    fs::create_dir_all(&bin).expect("isolated quality run area");
    let tama = installed_tama();
    std::os::unix::fs::symlink(&tama, bin.join("tama")).expect("place the installed Tama");
    let path = path_without_oko();

    let attested = commit(&root, &home, "attested", &attested_message(&tama));
    let unattested = commit(
        &root,
        &home,
        "unattested",
        "Fixture commit with HEAD's tree and no attestation\n",
    );
    let passed = quality(&root, &home, &path, &attested);
    let refused = quality(&root, &home, &path, &unattested);
    let report = json!({
        "source_revision": super::git(&root, &["rev-parse", "HEAD"]),
        "binary": env!("CARGO_BIN_EXE_stado"),
        "tama": tama,
        "path": path,
        "commands": [
            {"argv": ["stado", "quality", "check", "--root", root, "--revision", attested],
             "exit_status": passed.status.code(), "stdout": String::from_utf8_lossy(&passed.stdout),
             "stderr": String::from_utf8_lossy(&passed.stderr)},
            {"argv": ["stado", "quality", "check", "--root", root, "--revision", unattested],
             "exit_status": refused.status.code(), "stdout": String::from_utf8_lossy(&refused.stdout),
             "stderr": String::from_utf8_lossy(&refused.stderr)}
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
        "an attested tree was refused: {report}"
    );
    assert!(
        said.contains("CLI command tree was not judged")
            && said.contains("CLI command-tree judgment through Brama failed")
            && said.contains(&format!("stands on the attestation {attested} carries")),
        "the pass does not say the tree stood on its attestation: {report}"
    );
    let refusal = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "an unattested tree passed: {report}"
    );
    assert!(
        refusal.contains("CLI command-tree judgment through Brama failed")
            && refusal.contains(&format!("the attestation of {unattested} does not stand")),
        "the refusal does not name the unjudged tree and the missing attestation: {report}"
    );
}
