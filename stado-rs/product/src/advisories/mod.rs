//! `stado product advisories list|apply [PRODUCT]`: the published security
//! advisories that affect what the workspace's checkouts lock, and the
//! lockfile updates that close them.
//!
//! A product compiles in what its committed `Cargo.lock` resolves: a TLS
//! verifier, a root-certificate bundle, a parser. When an advisory is
//! published against one of those versions, every product locking it keeps
//! shipping the flaw until its lock moves. `list` reads what every lockfile
//! of the selected checkouts holds in their last commit against an OSV
//! advisory database and names each affected package version, the advisory,
//! the fixed versions and whether Cargo can reach a fix without a manifest
//! change; `apply` moves each one that can with `cargo update --precise`,
//! names every other package that moved with it, and names why each other
//! advisory cannot close. Neither commits: the changed lockfiles are the
//! checkout's, and a lockfile whose working file differs from its last
//! commit is named as such while its committed findings stand.

mod apply;
mod osv;
mod report;
mod scan;

use crate::common::Runtime;
use anyhow::{bail, Context, Result};
use clap::{Arg, ArgAction, Command};
use scan::Checkout;
use std::{fs, path::Path};

/// `stado product advisories`.
pub fn command() -> Command {
    Command::new("advisories")
        .about(
            "Read every committed Cargo.lock of the workspace's checkouts against an OSV \
             advisory database and name each affected package version, its advisory and fixed \
             versions (list), or lock each one Cargo can fix without a manifest change and say \
             why for the rest (apply)",
        )
        .arg(
            Arg::new("operation")
                .required(true)
                .value_parser(["list", "apply"])
                .help(
                    "list: report what stands in the committed locks; apply: cargo update \
                     --precise each affected package to its first caret-compatible fixed \
                     version, then report what stands in the committed locks",
                ),
        )
        .arg(Arg::new("positional").value_name("PRODUCT").help(
            "The checkouts of one catalog product; every git checkout directly in the workspace \
             when omitted",
        ))
        .arg(
            Arg::new("advisory")
                .long("advisory")
                .value_name("ID")
                .action(ArgAction::Append)
                .help("Only this advisory, by id or alias; repeatable"),
        )
        .arg(
            Arg::new("database")
                .long("database")
                .value_name("ORIGIN")
                .default_value(osv::PUBLIC_DATABASE)
                .help(
                    "The OSV database whose storage serves <ORIGIN>/crates.io/all.zip; the public \
                     OSV database when omitted",
                ),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print the checkouts, updates and standing findings as JSON"),
        )
}

/// Every git checkout directly in `workspace`, in name order.
fn workspace_checkouts(workspace: &Path) -> Result<Vec<Checkout>> {
    let mut checkouts = Vec::new();
    let entries = fs::read_dir(workspace)
        .with_context(|| format!("reading the workspace {}", workspace.display()))?;
    for entry in entries {
        let entry =
            entry.with_context(|| format!("reading the workspace {}", workspace.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .with_context(|| format!("reading what {} is", path.display()))?;
        if kind.is_dir() && crate::common::files::present(&path.join(".git"))? {
            checkouts.push(Checkout {
                checkout: path,
                repository: None,
                read: true,
                lockfiles: Vec::new(),
                uncommitted: Vec::new(),
            });
        }
    }
    checkouts.sort_by(|left, right| left.checkout.cmp(&right.checkout));
    Ok(checkouts)
}

/// The checkouts a product's catalog record names, or every git checkout
/// directly in the workspace when no product is named.
fn selected(runtime: &Runtime, product: Option<&String>) -> Result<Vec<Checkout>> {
    let Some(wanted) = product else {
        return workspace_checkouts(&runtime.workspace);
    };
    let catalog = crate::catalog::current(runtime)?;
    let record = catalog["products"]
        .as_array()
        .context("catalog products must be a list")?
        .iter()
        .find(|record| record["id"].as_str() == Some(wanted.as_str()))
        .with_context(|| {
            format!(
                "the catalog at {} names no product {wanted}",
                runtime.catalog.display()
            )
        })?;
    let mut checkouts = Vec::new();
    for repository in crate::dependencies::repositories(record) {
        let name = repository
            .rsplit('/')
            .next()
            .with_context(|| format!("{wanted}: repository {repository:?} names no checkout"))?;
        let checkout = runtime.workspace.join(name);
        let read = crate::common::files::present(&checkout.join(".git"))?;
        checkouts.push(Checkout {
            checkout,
            repository: Some(repository),
            read,
            lockfiles: Vec::new(),
            uncommitted: Vec::new(),
        });
    }
    Ok(checkouts)
}

/// Why the advisories `wanted` names cannot be read, when one of them is
/// no advisory the database publishes or one it withdrew.
fn selection_refusal(database: &osv::Database, wanted: &[String]) -> Option<String> {
    let mut unknown = Vec::new();
    let mut withdrawn = Vec::new();
    for id in wanted.iter().filter(|id| !database.knows(id)) {
        match database.withdrawal(id) {
            Some((advisory, at)) if advisory == id => withdrawn.push(format!("{id} at {at}")),
            Some((advisory, at)) => {
                withdrawn.push(format!("{id} (an alias of {advisory}) at {at}"))
            }
            None => unknown.push(id.as_str()),
        }
    }
    let mut refusals = Vec::new();
    if !unknown.is_empty() {
        refusals.push(format!(
            "the advisory database {} holds no {} advisory {}; name an id or alias it publishes",
            database.origin,
            osv::ECOSYSTEM,
            unknown.join(", ")
        ));
    }
    if !withdrawn.is_empty() {
        refusals.push(format!(
            "the advisory database {} withdrew {}: a withdrawn advisory affects no version, so \
             there is nothing to read or lock for it; drop that --advisory",
            database.origin,
            withdrawn.join(", ")
        ));
    }
    match refusals.is_empty() {
        true => None,
        false => Some(refusals.join("; ")),
    }
}

/// Run `stado product advisories list|apply [PRODUCT] [--advisory ID]...
/// [--database ORIGIN] [--json]`. The exit status is a failure while any
/// selected advisory still affects a lock, a checkout could not be read, or
/// an update was refused or failed.
pub fn run(arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let operation = arguments
        .get_one::<String>("operation")
        .context("advisories operation is missing")?;
    let database_origin = arguments
        .get_one::<String>("database")
        .context("--database has no value")?;
    let wanted: Vec<String> = arguments
        .get_many::<String>("advisory")
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    let mut checkouts = selected(runtime, arguments.get_one::<String>("positional"))?;
    let database = osv::Database::read(database_origin)?;
    if let Some(refusal) = selection_refusal(&database, &wanted) {
        bail!("{refusal}");
    }
    scan::lockfiles(&mut checkouts)?;
    let findings = scan::findings(&database, &checkouts, &wanted)?;
    let (updates, remaining) = match operation.as_str() {
        "list" => (None, findings),
        "apply" => {
            let updates = apply::run(&findings)?;
            scan::lockfiles(&mut checkouts)?;
            let remaining = scan::findings(&database, &checkouts, &wanted)?;
            (Some(updates), remaining)
        }
        other => bail!("unknown advisories operation {other}; use list or apply"),
    };
    let answer = report::Answer {
        operation,
        workspace: &runtime.workspace,
        database: &database.origin,
        advisories: &wanted,
        checkouts: &checkouts,
        updates: updates.as_deref(),
        findings: &remaining,
    };
    report::print(&answer, arguments.get_flag("json"))?;
    Ok(match answer.settled() {
        true => libc::EXIT_SUCCESS,
        false => libc::EXIT_FAILURE,
    })
}
