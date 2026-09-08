//! What the table cannot say in a column: the counts and the sentences under
//! it.
//!
//! Split out of `report.rs` when that file crossed the repository's length
//! limit. Every sentence here is about the population as a whole, which is why
//! none of them fits a per-artifact row.

use super::CarriedArtifact;

/// How many artifacts each trailer speaks for.
pub(super) struct Counts {
    /// Rows whose producing commit was resolved and is not reachable.
    pub(super) drifted: usize,
    /// Rows nothing here could resolve, because no local checkout answered.
    pub(super) unresolved: usize,
    /// Installed helper scripts: no release behind them, kept out of the table.
    pub(super) helpers: usize,
    /// Non-executable files the delivery path itself writes, such as the
    /// release-version marker.
    pub(super) markers: usize,
}

/// Print every sentence the table needs under it, in the order an operator
/// acts on them.
pub(super) fn print(
    target: &str,
    carried: &[CarriedArtifact],
    counts: &Counts,
    rows: usize,
    repository_missing: bool,
) {
    if repository_missing {
        println!(
            "\n{target}: no local checkout was found, so reachability is unknown rather than \
             answered; run this from the stado source tree to resolve it"
        );
    }
    if counts.drifted != usize::default() {
        println!(
            "{target}: {} of {rows} artifacts name a producing commit that is not reachable \
             from origin/main",
            counts.drifted
        );
    }
    if counts.unresolved != usize::default() {
        // Unknown, never folded into drift. An operator told "no" walks a
        // build back; one told "unknown" clones the repository first, and the
        // trailer used to say the first about artifacts nobody had asked
        // about.
        println!(
            "{target}: {} of {rows} artifacts could not be resolved against any checkout here, \
             which is unmeasured rather than unreachable",
            counts.unresolved
        );
    }
    let replaced = carried
        .iter()
        .filter(|item| item.describes == Some(false))
        .count();
    if replaced != usize::default() {
        // Louder than drift, because the record is not merely absent: it
        // answers the provenance question, and its answer is about bytes that
        // are gone. Every reader downstream inherits that wrong answer.
        println!(
            "{target}: {replaced} artifact(s) were replaced after their record was written, so \
             the commit shown for them describes bytes that are no longer on the host"
        );
    }
    if counts.helpers != usize::default() {
        // Not drift, and not nothing. Helpers are delivered one at a time to
        // solve one incident and are never removed, so the population only
        // grows; naming the count is what makes an operator notice that a
        // directory of them accumulated while nobody decided to keep any.
        println!(
            "{target}: {} installed helper script(s) alongside, which carry no release \
             and are not counted above",
            counts.helpers
        );
    }
    if counts.markers != usize::default() {
        // The release path's own version marker, and anything else in the bin
        // directory that is not executable. Named rather than listed: a file
        // the delivery wrote is not an artifact whose provenance can be
        // questioned, and a row for it can never be closed.
        println!(
            "{target}: {} non-executable file(s) alongside, written by the delivery path \
             itself and not counted above",
            counts.markers
        );
    }
}
