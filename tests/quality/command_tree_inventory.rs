//! `stado quality command-tree` lists the one-off commands of a committed
//! revision's built CLI instead of refusing it.
//!
//! The real `stado` reads Stado's own checkout at HEAD with an isolated home
//! under `.build/quality-command-tree-inventory/`: it builds the release
//! recipe's executable, reads its command tree and has Brama judge it, and
//! answers exit 0 with one document naming the product, the revision, that
//! it has a CLI, and `remaining`, one sentence per command. With every `oko`
//! left off `PATH` no judgment is obtained, and the inventory is refused,
//! non-zero, naming why rather than listing nothing. An object that is no
//! commit (HEAD's tree) is refused before anything is built. Invocations,
//! statuses and both streams are retained in the run's `report.json`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("git launch");
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout)
        .expect("git output is UTF-8")
        .trim()
        .to_owned()
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

fn inventory(root: &Path, home: &Path, path: &str, revision: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .env("HOME", home)
        .env("PATH", path)
        .args([
            "quality",
            "command-tree",
            "--root",
            root.to_str().expect("repository path"),
            "--revision",
            revision,
            "--json",
        ])
        .output()
        .expect("run the real command-tree inventory")
}

fn row(revision: &str, path: &str, output: &Output) -> Value {
    json!({
        "argv": ["stado", "quality", "command-tree", "--revision", revision, "--json"],
        "path": path,
        "exit_status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    })
}

#[test]
fn a_judged_tree_is_listed_and_an_unjudged_one_is_refused() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("Stado checkout")
        .to_path_buf();
    let home = root
        .join(".build/quality-command-tree-inventory")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&home).expect("isolated inventory run area");
    let head = git(&root, &["rev-parse", "HEAD"]);
    let tree = git(&root, &["rev-parse", "HEAD^{tree}"]);
    let path = std::env::var("PATH").expect("PATH is UTF-8");
    let without_oko = path_without_oko();

    let judged = inventory(&root, &home, &path, &head);
    let unjudged = inventory(&root, &home, &without_oko, &head);
    let no_commit = inventory(&root, &home, &path, &tree);
    let report = json!({
        "source_revision": head,
        "binary": env!("CARGO_BIN_EXE_stado"),
        "commands": [
            row(&head, &path, &judged),
            row(&head, &without_oko, &unjudged),
            row(&tree, &path, &no_commit),
        ],
    });
    let report_path = home.join("report.json");
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("serialize report"),
    )
    .expect("retain report");
    eprintln!("command-tree inventory evidence: {}", report_path.display());

    assert!(judged.status.success(), "a judged tree is listed: {report}");
    let document: Value = serde_json::from_slice(&judged.stdout).expect("one JSON document");
    assert_eq!(document["product"], "stado", "{document}");
    assert_eq!(document["revision"], head.as_str(), "{document}");
    assert_eq!(document["cli"], true, "{document}");
    let Some(remaining) = document["remaining"].as_array() else {
        panic!("remaining is a list: {document}");
    };
    assert!(
        remaining.iter().all(|line| line
            .as_str()
            .is_some_and(|line| line.starts_with("command `") && line.contains("primitive `"))),
        "each remaining entry names a command and its primitive: {document}"
    );

    // With --json a refusal is the error document on stdout.
    let refusal = String::from_utf8_lossy(&unjudged.stdout);
    assert!(
        !unjudged.status.success(),
        "an unjudged tree was listed: {report}"
    );
    assert!(
        refusal.contains("was not judged") && refusal.contains("no inventory"),
        "the refusal names the missing judgment: {report}"
    );

    assert!(
        !no_commit.status.success(),
        "a tree object was read as a commit: {report}"
    );
    assert!(
        String::from_utf8_lossy(&no_commit.stdout).contains(&tree),
        "the refusal names the object: {report}"
    );
}
