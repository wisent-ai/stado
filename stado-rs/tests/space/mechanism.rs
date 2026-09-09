//! What reaches the bytes outside the declared reclamation stage roots.
//!
//! Two mechanisms clean a Stado host: the stages `stado space reclaim` runs,
//! and the cleaners the janitor runs from the host's registry policy. The
//! coverage report knew about the first and printed every path outside it as a
//! path nothing looks at. On 2026-09-09 that was false on `charless-mac-mini`:
//! `~/.stado/local-storage` at 52.5 GiB and `~/.stado/local-backup` at
//! 10.5 GiB were reported that way while `release_store` and `backup_twins`,
//! which sweep exactly those roots, were declared on that host, and its
//! janitor's last pass had stopped at its own per-pass budget.
//!
//! Each case seeds real payloads on this machine's filesystem and reads the
//! report's own answer back: the mechanism named per row, the figure for what
//! nothing reaches, and the human lines an operator sees.

use std::fs;

use serde_json::Value;

use crate::fixture::{Host, TARGET};
use crate::system::said;

/// A `stado` recent enough for every cleaner in the catalogue.
const CURRENT: &str = "0.16.38";

/// The row the coverage section carries for `path`, if it carries one.
fn uncovered_row(coverage: &Value, path: &str) -> Value {
    coverage["uncovered"]
        .as_array()
        .expect("the coverage names its rows")
        .iter()
        .find(|row| row["path"] == path)
        .unwrap_or_else(|| panic!("{path} is not in the coverage: {coverage}"))
        .clone()
}

/// The report's coverage section for the fixture target.
fn coverage(host: &Host) -> Value {
    host.json(&["space", "report", TARGET, "--json"])["coverage"].clone()
}

/// The report names what a declared stage root covers and what nothing
/// covers, measured against payloads this case wrote.
///
/// The defect: on 2026-09-09 a full mini printed `99%` and `cap_reached`
/// while 52.4 GiB under `~/.stado/local-storage` sat where no stage looks,
/// and no reading said so. Both trees below carry the same bytes, so a report
/// that counted the undeclared one as covered fails here.
#[test]
fn the_report_names_covered_roots_and_what_nothing_covers() {
    let host = Host::new();
    let scratch = host.under_home(".stado/build-work");
    fs::create_dir_all(&scratch).expect("create the declared scratch root");
    host.seed_tree(&scratch, "release-tree", 200, false);
    let stranded = host.seed_tree(&host.home, "nothing-declares-this", 200, false);

    let coverage = coverage(&host);
    let covered = coverage["covered"]
        .as_array()
        .expect("the coverage names the declared roots")
        .iter()
        .find(|row| row["root"] == scratch.to_string_lossy().as_ref())
        .unwrap_or_else(|| panic!("the scratch root is not covered: {coverage}"));
    assert_eq!(covered["stage"], "build_scratch");
    assert_eq!(covered["measured"], true, "{covered}");
    assert!(
        covered["bytes"].as_i64().unwrap_or_default() > 0,
        "{covered}"
    );

    let named = uncovered_row(&coverage, &stranded.to_string_lossy());
    assert!(named["bytes"].as_i64().unwrap_or_default() > 0, "{named}");
    assert!(
        named["mechanism"].is_null(),
        "no cleaner sweeps a tree this case invented, so no mechanism may be named: {named}"
    );
    assert!(
        !coverage["verdict"].as_str().unwrap_or_default().is_empty(),
        "the coverage carries no verdict: {coverage}"
    );
}

/// A path a declared cleaner sweeps is reported with that cleaner's name, and
/// its bytes are not counted as bytes nothing reaches.
///
/// This is the sentence that was false on the mini. The declaration is written
/// by the product's own command, so the case covers the write and the reading
/// it changes in one flow.
#[test]
fn a_path_a_declared_cleaner_sweeps_is_named_with_that_cleaner() {
    let host = Host::new();
    host.declare_running(&host.policy(), CURRENT);
    let replicas = host.under_home(".stado/local-backup");
    fs::create_dir_all(&replicas).expect("create the replica root");
    host.seed_tree(&replicas, "object-twin", 200, false);

    // Before the declaration: the product implements a cleaner for this root
    // and the host arms none, so the report must name it and say what to run.
    let before = coverage(&host);
    let unarmed = before["unarmed"]
        .as_array()
        .expect("the coverage lists what could be armed")
        .iter()
        .find(|row| row["cleaner"] == "backup_twins")
        .unwrap_or_else(|| panic!("backup_twins is not named as unarmed: {before}"));
    assert_eq!(
        unarmed["root"],
        Value::from(replicas.to_string_lossy().to_string())
    );
    assert!(
        unarmed["bytes"].as_i64().unwrap_or_default() > 0,
        "the unarmed row must carry the bytes standing in its root: {unarmed}"
    );
    assert_eq!(unarmed["supported_by_installed_binary"], Value::Bool(true));
    assert!(
        before["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("stado space cleaners declare"),
        "the verdict must name the command that arms it: {before}"
    );
    let row_before = uncovered_row(&before, &replicas.to_string_lossy());
    assert_eq!(row_before["mechanism"], "backup_twins");
    assert_eq!(row_before["mechanism_declared"], Value::Bool(false));

    host.json(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "backup_twins",
        "--json",
    ]);

    let after = coverage(&host);
    let row = uncovered_row(&after, &replicas.to_string_lossy());
    assert_eq!(row["mechanism"], "backup_twins");
    assert_eq!(
        row["mechanism_declared"],
        Value::Bool(true),
        "the declaration the command just wrote must reach the reading: {row}"
    );
    let bytes = row["bytes"].as_i64().unwrap_or_default();
    assert!(bytes > 0, "{row}");
    assert_eq!(
        after["unswept_bytes"].as_i64(),
        after["uncovered_bytes"]
            .as_i64()
            .zip(after["cleaner_bytes"].as_i64())
            .map(|(outside, swept)| outside - swept),
        "the figure for what nothing reaches must exclude what a declared cleaner sweeps: {after}"
    );
    assert!(
        after["cleaner_bytes"].as_i64().unwrap_or_default() >= bytes,
        "these bytes are swept by a declared cleaner and must be counted as such: {after}"
    );
    assert!(
        after["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("is swept by the declared cleaner `backup_twins`"),
        "the verdict must say a pass reaches those bytes: {after}"
    );
}

/// The tab-delimited console row for `path`.
///
/// The verdict sentence names the same path, so a naive substring search
/// matches the prose instead of the row it is about.
fn row_for<'a>(lines: &'a str, path: &str) -> &'a str {
    lines
        .lines()
        .find(|line| line.split('\t').next_back() == Some(path))
        .unwrap_or_else(|| panic!("{path} has no row in the console report:\n{lines}"))
}

/// The human report labels each row with the mechanism, so `uncovered` beside
/// a path a declared cleaner sweeps cannot be printed again.
#[test]
fn the_console_rows_name_the_mechanism_rather_than_the_word_uncovered() {
    let host = Host::new();
    host.declare_running(&host.policy(), CURRENT);
    let replicas = host.under_home(".stado/local-backup");
    fs::create_dir_all(&replicas).expect("create the replica root");
    host.seed_tree(&replicas, "object-twin", 200, false);
    let stranded = host.seed_tree(&host.home, "nothing-declares-this", 200, false);

    let unarmed = host.run(&["space", "report", TARGET]);
    assert!(unarmed.status.success(), "{}", said(&unarmed.stderr));
    let lines = said(&unarmed.stdout);
    assert!(
        row_for(&lines, &replicas.to_string_lossy()).starts_with("unarmed:backup_twins\t"),
        "an implemented cleaner nobody declared must be labelled as unarmed:\n{lines}"
    );

    host.json(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "backup_twins",
        "--json",
    ]);

    let armed = host.run(&["space", "report", TARGET]);
    assert!(armed.status.success(), "{}", said(&armed.stderr));
    let lines = said(&armed.stdout);
    let replica_line = row_for(&lines, &replicas.to_string_lossy());
    assert!(
        replica_line.starts_with("backup_twins\t"),
        "the row must open with the cleaner that sweeps it: {replica_line}"
    );
    let stranded_line = row_for(&lines, &stranded.to_string_lossy());
    assert!(
        stranded_line.starts_with("uncovered\t"),
        "a path no mechanism reaches must still read as uncovered: {stranded_line}"
    );
}
