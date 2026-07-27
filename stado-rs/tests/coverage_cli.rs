//! `stado-coverage` CLI tests: list with the (empty) static registry and
//! the unknown-universe UsageError. The crate ships no universes (they
//! are external plugins in Python too), so every universe id is unknown
//! to the binary.

use std::process::{Command, Output};

fn coverage(args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado-coverage"));
    cmd.args(args);
    cmd.output().expect("stado-coverage binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn list_with_empty_registry_exits_1() {
    let out = coverage(&["list"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert_eq!(stdout(&out), "");
    assert_eq!(stderr(&out), "(no universes registered)\n");
}

#[test]
fn unknown_universe_is_a_click_usage_error() {
    let out = coverage(&["verify", "nosuch"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert_eq!(
        stderr(&out),
        "Usage: stado-coverage verify [OPTIONS] UNIVERSE_ID\n\
         Try 'stado-coverage verify --help' for help.\n\
         \n\
         Error: unknown universe 'nosuch'. Registered: (none)\n"
    );

    let out = coverage(&["retry", "nosuch", "--execute"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("Usage: stado-coverage retry [OPTIONS] UNIVERSE_ID\n"));
    assert!(stderr(&out).contains("Error: unknown universe 'nosuch'. Registered: (none)\n"));
}

#[test]
fn bad_kv_is_a_click_usage_error() {
    let out = coverage(&["verify", "nosuch", "--kv", "no-equals"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("Error: --kv expects KEY=VALUE, got 'no-equals'\n"));
}
