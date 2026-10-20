//! A Clap CLI's leaves are read from Clap's own help grammar, never sent to
//! the model. The real quality check runs over the committed Stado tree with
//! the real Oko on the path behind a pass-through recorder: every model call
//! still reaches Brama, and the recorder only keeps what was asked. The tree
//! verdict must be asked; the per-command discovery question must not be
//! asked even once, because every Stado command is a Clap command.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

/// The Oko program the quality check calls for every model judgement.
const OKO: &str = "oko";
/// Words only the per-command discovery prompt carries.
const DISCOVERY_PROMPT: &str = "listing only direct, advertised subcommand tokens";
/// Words only the whole-tree verdict prompt carries.
const VERDICT_PROMPT: &str = "Judge the real command tree below";

/// The installed Oko the shell would run.
fn real_oko() -> PathBuf {
    let path = env::var_os("PATH").expect("PATH names where Oko is installed");
    env::split_paths(&path)
        .map(|directory| directory.join(OKO))
        .find(|candidate| candidate.is_file())
        .expect("Oko is installed on PATH; the quality check asks the model through it")
}

/// A recorder named `oko` that appends each call's arguments to `calls` and
/// then runs the real Oko with them unchanged.
fn recorder(bin: &Path, real: &Path, calls: &Path) {
    fs::create_dir_all(bin).expect("recorder directory");
    let script = bin.join(OKO);
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            calls.display(),
            real.display()
        ),
    )
    .expect("write recorder");
    let permissions = fs::metadata(real)
        .expect("read the real Oko's mode")
        .permissions();
    fs::set_permissions(&script, permissions).expect("make recorder executable");
}

#[test]
fn clap_leaves_are_not_sent_to_the_model() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("Stado checkout")
        .to_path_buf();
    let home = root
        .join(".build/quality-command-tree")
        .join(uuid::Uuid::new_v4().to_string());
    let bin = home.join("bin");
    let calls = home.join("oko-calls.log");
    recorder(&bin, &real_oko(), &calls);
    let path = env::join_paths(
        std::iter::once(bin.clone()).chain(env::split_paths(&env::var_os("PATH").expect("PATH"))),
    )
    .expect("PATH with the recorder first");
    let revision = super::git(&root, &["rev-parse", "HEAD"]);

    let run = Command::new(env!("CARGO_BIN_EXE_stado"))
        .env("HOME", &home)
        .env("PATH", &path)
        .args(["quality", "check", "--root"])
        .arg(&root)
        .args(["--revision", &revision])
        .output()
        .expect("run the real quality check");
    let asked = fs::read_to_string(&calls).expect("the recorder saw the model calls");
    let report = json!({
        "source_revision": revision,
        "binary": env!("CARGO_BIN_EXE_stado"),
        "argv": ["stado", "quality", "check", "--root", root, "--revision", revision],
        "exit_status": run.status.code(),
        "stdout": String::from_utf8_lossy(&run.stdout),
        "stderr": String::from_utf8_lossy(&run.stderr),
        "discovery_prompts": asked.matches(DISCOVERY_PROMPT).count(),
        "tree_verdict_prompts": asked.matches(VERDICT_PROMPT).count(),
    });
    let report_path = home.join("leaf-report.json");
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("serialize report"),
    )
    .expect("retain report");
    eprintln!("CLI leaf discovery evidence: {}", report_path.display());

    assert!(run.status.success(), "quality check refused: {report}");
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("CLI command tree passed"),
        "no real tree verdict: {report}"
    );
    assert!(
        asked.contains(VERDICT_PROMPT),
        "the tree verdict never reached Oko: {report}"
    );
    assert!(
        !asked.contains(DISCOVERY_PROMPT),
        "Clap commands were sent to the model for discovery: {report}"
    );
}
