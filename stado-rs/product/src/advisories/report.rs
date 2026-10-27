//! The answer of `stado product advisories`, as JSON or as lines for a person,
//! from the same data.

use super::{
    apply::{listed, Moved, Update},
    scan::{Checkout, Finding},
};
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

/// What one invocation read and did.
#[derive(Serialize)]
pub struct Answer<'a> {
    pub operation: &'a str,
    pub workspace: &'a Path,
    pub database: &'a str,
    /// The advisories `--advisory` selected; every advisory when empty.
    pub advisories: &'a [String],
    pub checkouts: &'a [Checkout],
    /// `apply` only: what was done for each affected package version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updates: Option<&'a [Update]>,
    /// The findings that stand in the committed lockfiles when the command
    /// ends: for `apply`, read again after the updates.
    pub findings: &'a [Finding],
}

impl Answer<'_> {
    /// Nothing selected still affects a committed lock, every checkout was
    /// read, and every update asked for was made, by it or by an earlier
    /// update of the same lockfile.
    pub fn settled(&self) -> bool {
        self.checkouts.iter().all(|checkout| checkout.read)
            && !self.findings.iter().any(Finding::actionable)
            && self
                .updates
                .into_iter()
                .flatten()
                .all(|update| update.state.made())
    }
}

fn lockfile_path(checkout: &Path, lockfile: &str) -> String {
    checkout.join(lockfile).display().to_string()
}

/// `text` after `prefix` when there is text; nothing when there is none.
fn part(prefix: &str, text: Option<&str>) -> String {
    match text {
        Some(text) => format!("{prefix}{text}"),
        None => String::new(),
    }
}

fn unread(checkout: &Checkout) -> String {
    let named = part(" of ", checkout.repository.as_deref());
    format!(
        "checkout {}{named} is absent, so what its lockfiles resolve is unknown",
        checkout.checkout.display()
    )
}

fn moved_part(moved: &Moved) -> String {
    format!(
        "{} {} -> {}",
        moved.package,
        listed(moved.from.iter().map(String::as_str)),
        listed(moved.to.iter().map(String::as_str)),
    )
}

fn update_line(update: &Update) -> String {
    let also = match update.moved.is_empty() {
        true => String::new(),
        false => format!(
            "; cargo also moved {}",
            update
                .moved
                .iter()
                .map(moved_part)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    format!(
        "{} {}: {} {}{} ({}){also}",
        update.state.word(),
        lockfile_path(&update.checkout, &update.lockfile),
        update.package,
        update.from,
        part(" -> ", update.to.as_deref()),
        update.advisories.join(", "),
    )
}

fn finding_line(finding: &Finding) -> String {
    let remedy = match &finding.compatible_fix {
        Some(fix) => format!("cargo can lock {fix} without a manifest change"),
        None => format!(
            "no caret-compatible fix; depended on by {}",
            finding.dependents.join(", ")
        ),
    };
    let notice = match finding.informational.as_deref() {
        Some(kind) => format!(" [informational: {kind}]"),
        None => String::new(),
    };
    let working = match &finding.working {
        Some(versions) => format!(
            "; its working file, not committed, locks {}",
            listed(versions.iter().map(String::as_str))
        ),
        None => String::new(),
    };
    format!(
        "{}: {} {}: {}{notice}{}; fixed in {}; {remedy}{working}",
        lockfile_path(&finding.checkout, &finding.lockfile),
        finding.package,
        finding.version,
        finding.advisory,
        part(" ", finding.summary.as_deref()),
        finding.fixed.join(", "),
    )
}

/// Print `answer`; refusals and unread checkouts go to standard error.
pub fn print(answer: &Answer<'_>, json: bool) -> Result<()> {
    for checkout in answer.checkouts.iter().filter(|checkout| !checkout.read) {
        eprintln!("{}", unread(checkout));
    }
    if json {
        return crate::common::emit(&serde_json::to_value(answer)?);
    }
    for update in answer.updates.into_iter().flatten() {
        match &update.cause {
            Some(cause) => eprintln!("{}: {cause}", update_line(update)),
            None => println!("{}", update_line(update)),
        }
    }
    for checkout in answer.checkouts {
        for lockfile in &checkout.uncommitted {
            println!(
                "{} differs from its last commit; the findings read the committed one",
                lockfile_path(&checkout.checkout, lockfile)
            );
        }
    }
    for finding in answer.findings {
        println!("{}", finding_line(finding));
    }
    let lockfiles: usize = answer
        .checkouts
        .iter()
        .map(|checkout| checkout.lockfiles.len())
        .sum();
    let (actionable, notices): (Vec<&Finding>, Vec<&Finding>) = answer
        .findings
        .iter()
        .partition(|finding| finding.actionable());
    println!(
        "{} advisory finding(s) and {} notice(s) stand in {lockfiles} committed lockfile(s) of \
         {} checkout(s), read against {}",
        actionable.len(),
        notices.len(),
        answer.checkouts.len(),
        answer.database,
    );
    Ok(())
}
