//! The software report `release host-state` leaves on file for
//! `release status`.
//!
//! Until 2026-09-06 `stado host software` wrote this report. The change that
//! collapsed the host verbs into the release capability deleted that verb and
//! kept everything it fed: four days later every rollout target read
//! `reported stale (4d)` and the sentence beside it sent operators to a
//! command that answered `Usage: stado host <COMMAND>`. The two cases here
//! are the two halves of that regression — the report has a writer, and the
//! sentence names a command this binary parses.
//!
//! The host is this machine, under a home directory this test owns: the
//! product reads `$HOME/.stado/bin` and writes `$HOME/.stado/observations.json`,
//! and both have to be the test's so a run neither reads the operator's
//! programs nor writes into the operator's observation store.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::fixture::{installed_binary, installed_version, report, stderr, Fixture, TARGET};

/// A home of this test's own, carrying the one managed binary this machine
/// really has, hard-linked so the product reads the real bytes.
struct Home {
    root: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("an isolated home directory");
        let bin = root.path().join(".stado").join("bin");
        std::fs::create_dir_all(&bin).expect("create the isolated bin directory");
        let source = installed_binary();
        let destination = bin.join("stado");
        if std::fs::hard_link(&source, &destination).is_err() {
            std::fs::copy(&source, &destination).expect("copy the managed binary into the home");
        }
        Self { root }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn binary(&self) -> PathBuf {
        self.path().join(".stado").join("bin").join("stado")
    }

    fn observations(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.path().join(".stado").join("observations.json"))
            .expect("host-state leaves an observation store in the home it read");
        serde_json::from_str(&text).expect("the observation store stays valid JSON")
    }
}

#[test]
fn host_state_writes_the_software_report_release_status_reads() {
    let Some(version) = installed_version() else {
        panic!(
            "no managed binary at {} to read a version from",
            installed_binary().display()
        );
    };
    let fixture = Fixture::new();
    let home = Home::new();
    let declared = fixture.declare(&version);
    assert!(declared.status.success(), "{}", stderr(&declared));

    let output = fixture.host_state_in_home(home.path(), &[]);
    let report = report(&output);
    // The home holds no staged copy of the binary, so the drift gate refuses
    // the bytes as unattested and the command exits non-zero. The report is
    // written on that visit all the same: a refresh that only happened when
    // the verdict was clean would leave `release status` stale on exactly
    // the hosts it most needs to describe.
    assert!(!output.status.success(), "{}", stderr(&output));
    assert_eq!(report["binaries"][0]["verdict"], "unattested");

    // What the command says it put on file.
    let software = &report["software"];
    assert_eq!(software["host"], TARGET);
    assert_eq!(software["state"], "observed");
    assert_eq!(software["observed"], "just now");
    let programs = software["programs"]
        .as_array()
        .expect("the software block lists the programs it rowed");
    let stado = programs
        .iter()
        .find(|program| program["name"] == "stado")
        .expect("the managed binary in the home's bin directory is rowed");
    assert_eq!(stado["path"], home.binary().to_string_lossy().as_ref());
    assert_eq!(
        stado["version"], version,
        "the row carries the version the binary prints"
    );
    assert_eq!(
        stado["provenance"], "unmanaged",
        "a home with no staged release has nothing to attest the bytes against"
    );

    // What is actually on file, which is what `release status` judges next.
    let rows = home.observations();
    let roster = rows
        .iter()
        .find(|row| row["fact"] == format!("software-report:{TARGET}"))
        .expect("the report's own roster row is on file");
    assert_eq!(roster["vantage"], TARGET);
    assert_eq!(roster["state"], "observed");
    let row = rows
        .iter()
        .find(|row| row["fact"] == format!("software:stado@{TARGET}"))
        .expect("the managed binary's row is on file");
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains(&format!("version={version} ")),
        "the stored row carries the version: {detail}"
    );
    assert!(
        detail.ends_with(&format!("path={}", home.binary().display())),
        "the stored row names the file it read: {detail}"
    );
}

#[test]
fn the_sentence_that_asks_for_a_refresh_names_a_command_this_binary_parses() {
    let finding = stado::host_software::judge(
        &stado::host_software::Report::never(TARGET),
        &Default::default(),
        None,
    );
    assert!(finding.failed);
    let sentence = finding
        .sentences
        .first()
        .expect("a host that never reported is a finding");
    let command = sentence
        .rsplit_once("run `")
        .and_then(|(_, tail)| tail.strip_suffix('`'))
        .unwrap_or_else(|| panic!("the sentence ends by naming a command: {sentence}"));
    let mut words = command.split_whitespace();
    assert_eq!(words.next(), Some("stado"));
    let args: Vec<&str> = words.collect();
    assert!(
        args.contains(&TARGET),
        "the command names the host it is about: {command}"
    );

    // `--help` is the one invocation that proves the verb parses without
    // visiting a host: an unknown subcommand prints the parent's usage and
    // exits non-zero, which is exactly what `stado host software` did.
    let fixture = Fixture::new();
    let mut probe = args.clone();
    probe.push("--help");
    let output = fixture.stado(&probe);
    assert!(
        output.status.success(),
        "`{command} --help` must parse: {}",
        stderr(&output)
    );
}
