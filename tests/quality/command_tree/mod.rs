//! The committed Stado release recipe builds its actual executable. Quality
//! reads that binary's advertised command tree and obtains Brama's semantic
//! verdict. A historical committed tree with the duplicate janitor verb must
//! be refused with the command and the general space primitive named.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;
use sha2::{Digest, Sha256};

mod hyphenated;
mod leaf;
mod unjudged;

/// A run area under `root` that stands in for HOME, holding the operator's
/// Rust toolchain where the product finds it (`~/.cargo/bin/cargo`, and the
/// toolchains rustup reads from `~/.rustup`): the release build compiles with
/// it, and an isolated home without it refuses the check before any gate.
fn isolated_home(root: &Path) -> PathBuf {
    let home = root
        .join(".build/quality-command-tree")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&home).expect("isolated quality run area");
    let operator = PathBuf::from(std::env::var_os("HOME").expect("HOME names the operator's home"));
    for toolchain in [".cargo", ".rustup"] {
        std::os::unix::fs::symlink(operator.join(toolchain), home.join(toolchain))
            .expect("place the operator's Rust toolchain in the isolated home");
    }
    home
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("git launch");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output is UTF-8")
        .trim()
        .to_owned()
}

fn quality(root: &Path, home: &Path, revision: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .env("HOME", home)
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
fn committed_cli_tree_and_historical_duplicate() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("Stado checkout")
        .to_path_buf();
    let home = isolated_home(&root);
    let binary_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(env!("CARGO_BIN_EXE_stado")).expect("read tested binary"))
    );
    let current = git(&root, &["rev-parse", "HEAD"]);
    let change = git(
        &root,
        &[
            "log",
            "-1",
            "--format=%H",
            "-S",
            "DiskCleanup {",
            "--",
            "stado-rs/src/cli/entry/spec/root/installation.rs",
        ],
    );
    assert!(
        !change.is_empty(),
        "history must contain the janitor verb cutover"
    );
    let earlier = git(&root, &["rev-parse", &format!("{change}^")]);

    let before = quality(&root, &home, &earlier);
    let after = quality(&root, &home, &current);
    let report = json!({
        "source_revision": current,
        "historical_revision": earlier,
        "binary": env!("CARGO_BIN_EXE_stado"),
        "binary_sha256": binary_sha256,
        "commands": [
            {"argv": ["stado", "quality", "check", "--root", root, "--revision", earlier],
             "exit_status": before.status.code(), "stdout": String::from_utf8_lossy(&before.stdout),
             "stderr": String::from_utf8_lossy(&before.stderr)},
            {"argv": ["stado", "quality", "check", "--root", root, "--revision", current],
             "exit_status": after.status.code(), "stdout": String::from_utf8_lossy(&after.stdout),
             "stderr": String::from_utf8_lossy(&after.stderr)}
        ]
    });
    let report_path = home.join("report.json");
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("serialize report"),
    )
    .expect("retain report");
    eprintln!("CLI command-tree evidence: {}", report_path.display());

    let refusal = String::from_utf8_lossy(&before.stderr);
    assert!(!before.status.success(), "old duplicate passed: {report}");
    assert!(
        refusal.contains("disk-cleanup") && refusal.contains("space reclaim"),
        "old duplicate was not named with its primitive: {report}"
    );
    assert!(
        after.status.success(),
        "candidate CLI audit refused: {report}"
    );
    assert!(
        String::from_utf8_lossy(&after.stdout).contains("CLI command tree passed"),
        "no real tree verdict: {report}"
    );
}
