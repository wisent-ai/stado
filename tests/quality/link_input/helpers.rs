//! Running git in the fixture's checkout and reading what a check left in
//! the quality run areas.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

pub(super) fn git(directory: &Path, args: &[&str]) {
    // The cargo runner limits git to network transports for dependency
    // fetches; this fixture's origin is a directory beside the checkout.
    let status = Command::new("git")
        .env_remove("GIT_ALLOW_PROTOCOL")
        .args(args)
        .current_dir(directory)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} in {}", directory.display());
}

pub(super) fn git_output(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// What a check left in the checkout's quality run areas.
pub(super) fn left_behind(runs: &Path) -> Vec<String> {
    match fs::read_dir(runs) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("cannot read the quality scratch directory: {error}"),
    }
}
