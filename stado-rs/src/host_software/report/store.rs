//! Where a report is kept: the roster row, the write, and the read back.

use std::collections::BTreeSet;
use std::io;

use crate::host_software::{report_fact, software_fact, HostSoftware, REPORT_KIND};
use crate::observations::{self, Freshness, Observation, OBSERVED, UNVERIFIED};

use super::Report;

// ---------------------------------------------------------------------------
// Keeping the newest report
// ---------------------------------------------------------------------------

/// The roster row's detail: what the newest report listed, so a later read can
/// tell the report apart from every row ever written for this host.
fn roster_detail(names: &[&str], scripts: usize) -> String {
    format!(
        "reported={} scripts={scripts} names={}",
        names.len(),
        names.join(",")
    )
}

/// Persist one host's report, replacing whatever was on file for it.
///
/// Written through [`observations::record`], the file that already answers "when
/// did anyone last look" for this fleet, because a second store for the same kind
/// of fact is a second answer to that question. The roster row is what makes
/// replacement expressible in a store that merges and never deletes: a program
/// dropped from the newest report is dropped from the roster, so it stops being
/// part of the report even though its own row is still on file.
///
/// The vantage is the target, not this machine. The look happened on that host —
/// its files, its digests, its programs — and recording an operator's laptop as
/// the vantage would let two operators' runs overwrite each other's evidence
/// about a third machine.
pub fn record(host: &str, rows: &[HostSoftware], scripts: usize) -> io::Result<()> {
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    let mut written: Vec<Observation> = rows
        .iter()
        .map(|row| Observation::now(software_fact(&row.name, host), host, OBSERVED, row.detail()))
        .collect();
    written.push(Observation::now(
        report_fact(host),
        host,
        OBSERVED,
        roster_detail(&names, scripts),
    ));
    observations::record(&written)
}

/// Record that the look could not happen, in the channel's own words.
///
/// A failed read is written and not swallowed, because the alternative leaves the
/// previous report on file looking current — the exact shape of the twelve-day
/// outage [`crate::observations`] was built against. The roster keeps the names it
/// had, so the last thing anyone saw is still readable and is now visibly
/// unverified.
pub fn record_refusal(host: &str, detail: &str) -> io::Result<()> {
    let held = load(host);
    let names: Vec<&str> = held.rows.iter().map(|row| row.name.as_str()).collect();
    observations::record(&[Observation::now(
        report_fact(host),
        host,
        UNVERIFIED,
        format!("{} {detail}", roster_detail(&names, held.scripts)),
    )])
}

/// The roster row for one host: its freshness, the program names it listed, and
/// the script count beside them.
fn roster(records: &[Observation], host: &str) -> Option<(Freshness, BTreeSet<String>, usize)> {
    let fact = report_fact(host);
    let freshness = observations::freshness_in(records, &fact, observations::DEFAULT_TTL);
    let row = match &freshness {
        Freshness::Fresh(row) | Freshness::Stale(row) => row.clone(),
        Freshness::Never => return None,
    };
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut scripts = usize::default();
    for token in row.detail.split_whitespace() {
        if let Some(value) = token.strip_prefix("names=") {
            names.extend(
                value
                    .split(',')
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
            );
        } else if let Some(value) = token.strip_prefix("scripts=") {
            scripts = value.parse().unwrap_or_default();
        }
    }
    Some((freshness, names, scripts))
}

/// The newest report on file for one host.
pub fn load(host: &str) -> Report {
    load_in(&observations::load(), host)
}

/// [`load`] against records already in hand, for a reader asking about every
/// target in one rendering — the same reason [`observations::describe_in`]
/// exists.
pub fn load_in(records: &[Observation], host: &str) -> Report {
    let Some((freshness, names, scripts)) = roster(records, host) else {
        return Report::never(host);
    };
    let rows: Vec<HostSoftware> = names
        .iter()
        .filter_map(|name| {
            let fact = software_fact(name, host);
            records
                .iter()
                .filter(|row| row.fact == fact)
                .max_by(|left, right| left.at.cmp(&right.at))
                .and_then(|row| HostSoftware::from_detail(name, &row.detail))
        })
        .collect();
    Report {
        host: host.to_string(),
        rows,
        scripts,
        freshness,
    }
}

/// Every host that has a software report on file.
pub fn reported_hosts(records: &[Observation]) -> Vec<String> {
    let mut hosts: Vec<String> = records
        .iter()
        .filter_map(|row| row.fact.strip_prefix(REPORT_KIND).map(str::to_string))
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}
