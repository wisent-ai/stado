//! The readable run name derived from a submission's commands when the
//! submitter named none (`WC_RUN_NAME`).
//!
//! It is read from the structure every command has — the program it runs, its
//! first word — and not from flags a particular kind of workload happens to
//! take: a name built from `--model` and `--task` described one family of
//! commands and said nothing about the rest.

use std::path::Path;

/// The program a command runs: its first word, without its directory.
fn program(command: &str) -> Option<&str> {
    let first = command.split_whitespace().next()?;
    Path::new(first).file_name().and_then(|name| name.to_str())
}

/// `<program>:<N>jobs` when every command runs one program,
/// `<a>+<b>+…:<N>jobs` naming each distinct program in first-seen order
/// otherwise, and `<N>jobs` for commands that name no program.
pub fn derive_run_name(commands: &[String]) -> String {
    let mut programs: Vec<&str> = Vec::new();
    for name in commands.iter().filter_map(|command| program(command)) {
        if !programs.contains(&name) {
            programs.push(name);
        }
    }
    let jobs = format!("{}jobs", commands.len());
    if programs.is_empty() {
        return jobs;
    }
    format!("{}:{jobs}", programs.join("+"))
}
