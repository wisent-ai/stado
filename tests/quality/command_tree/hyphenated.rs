//! A command named as one hyphenated word is refused from the tree itself.
//!
//! The built Stado binary's own help is walked through Clap's `Commands:`
//! sections to find every command whose name holds a hyphen. The real
//! `stado quality tree --json` over this checkout must then list each
//! of them with the structural sentence, naming the command and the words it
//! should be split into, beside whatever the model reads. The invocation, its
//! status and both streams are retained under `.build/quality-command-tree/`.
//! Needs the release build toolchain and Oko with Brama for the model reading.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

/// Clap's header for the subcommands a command advertises.
const COMMANDS: &str = "Commands:";
/// The subcommand Clap generates for help, which is not a product command.
const HELP: &str = "help";

/// The direct subcommands `stado <path> --help` advertises.
fn children(path: &[String]) -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(path)
        .arg("--help")
        .output()
        .expect("run stado --help");
    let help = String::from_utf8(output.stdout).expect("help is UTF-8");
    let mut lines = help.lines();
    if !lines.by_ref().any(|line| line == COMMANDS) {
        return Vec::new();
    }
    lines
        .take_while(|line| line.starts_with(' '))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != HELP)
        .map(str::to_owned)
        .collect()
}

/// Every command path under `path` whose own name holds a hyphen.
fn hyphenated(path: &mut Vec<String>, found: &mut Vec<String>) {
    for child in children(path) {
        path.push(child.clone());
        if child.contains('-') {
            found.push(format!("stado {}", path.join(" ")));
        }
        hyphenated(path, found);
        path.pop();
    }
}

#[test]
fn hyphenated_command_names_are_refused_from_the_tree() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("Stado checkout")
        .to_path_buf();
    let mut found = Vec::new();
    hyphenated(&mut Vec::new(), &mut found);

    let home = super::isolated_home(&root);
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .env("HOME", &home)
        .args(["quality", "tree", "--root"])
        .arg(&root)
        .arg("--json")
        .output()
        .expect("run stado quality tree");
    let report = json!({
        "source_revision": super::git(&root, &["rev-parse", "HEAD"]),
        "hyphenated_in_help": found,
        "argv": ["stado", "quality", "tree", "--root", root, "--json"],
        "exit_status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    });
    let report_path: &Path = &home.join("hyphenated-report.json");
    fs::write(report_path, serde_json::to_vec_pretty(&report).expect("serialize report"))
        .expect("retain report");
    eprintln!("hyphenated command evidence: {}", report_path.display());

    assert!(output.status.success(), "the inventory was not given: {report}");
    let inventory: Value = serde_json::from_slice(&output.stdout).expect("inventory is JSON");
    let remaining: Vec<&str> = inventory["remaining"]
        .as_array()
        .expect("inventory lists what remains")
        .iter()
        .map(|line| line.as_str().expect("each remaining command is a sentence"))
        .collect();
    for command in &found {
        let name = command.rsplit(' ').next().expect("a command has a name");
        let sentence = format!(
            "command `{command}` names `{name}` as one hyphenated word: name it noun then verb, one word each (`{}`)",
            name.replace('-', " ")
        );
        assert!(
            remaining.contains(&sentence.as_str()),
            "{command} is not refused as a hyphenated name: {report}"
        );
    }
}
