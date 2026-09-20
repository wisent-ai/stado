//! `stado host disk-cleanup` against the real built binary.
//!
//! The janitor that enforces a host's free-space watermarks existed only as
//! `stado disk-cleanup`, which acts on the machine it is typed on. On
//! 2026-09-20 that left the fleet's only browser host below its low
//! watermark with no way back: charless-mac-mini published 6.7 GiB free
//! against 8 GiB, and Stado refused every placement on it with
//! `disk_pressure_active`, including the deployment that would have fixed
//! the host.
//!
//! Two properties are defended here, and both are read off a real run of the
//! real binary. An unknown target is named and refused before any channel is
//! opened. A dry run plans a pass and deletes nothing, which is what keeps
//! this command from quietly becoming a delete on whatever host it resolves.

use std::process::{Command, Output};

/// The binary under test, built by cargo for this integration target.
const STADO: &str = env!("CARGO_BIN_EXE_stado");

fn run(arguments: &[&str]) -> Output {
    Command::new(STADO)
        .args(arguments)
        .output()
        .expect("the built stado binary must run")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn an_unknown_target_is_named_and_refused() {
    let output = run(&["host", "disk-cleanup", "not-in-registry"]);
    assert!(
        !output.status.success(),
        "an unknown target must not exit zero: {}",
        stderr(&output)
    );
    assert_eq!(
        stderr(&output).lines().next(),
        Some("Error: target 'not-in-registry' is not in the canonical registry"),
        "the refusal must name the target it could not resolve: {}",
        stderr(&output)
    );
}

#[test]
fn the_help_declares_the_pass_it_runs_and_the_dry_run_that_deletes_nothing() {
    let output = run(&["host", "disk-cleanup", "--help"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let help = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        help.contains("--dry-run"),
        "the command must offer the pass that deletes nothing: {help}"
    );
    assert!(
        help.contains("registry-authorized"),
        "the command must say whose policy decides what is deleted: {help}"
    );
}
