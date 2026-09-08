//! Asking the host itself what it runs.
//!
//! [`gather`] is the pass: it decides the population and overlaps the readings.
//! [`Reporter`] is what takes one reading, and [`reporter`] holds those
//! readings. [`hasher`] resolves the host's SHA-256 tool, [`queries`] resolves
//! the version commands the shipped product catalog declares, and `parse`
//! decodes a reporter's stdout back into rows.

mod hasher;
mod parse;
mod queries;
mod reporter;

use std::collections::{BTreeMap, BTreeSet};

use futures::{stream, StreamExt};

use crate::deploy::service;
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::HostSoftware;
use hasher::Hasher;
use queries::{version_queries, VersionQuery};

pub use parse::parse;

// ---------------------------------------------------------------------------
// Reading the host
// ---------------------------------------------------------------------------

/// Enough parallelism to amortize an SSH round trip without opening an
/// unbounded number of sessions against one host.
const INSPECTION_CONCURRENCY: usize = 8;

/// The unit files TARGET declares, as `(kind, path)` pairs for the reporter.
///
/// The unit files come from the registry, because declarations live on the
/// control plane. The host is asked to read files and hash bytes; it is never
/// asked which of its files matter, which is how a reporter ends up carrying
/// an opinion the registry never authorized.
fn declared_units(target: &ComputeTarget) -> Vec<(String, String)> {
    service::declared_services(target)
        .into_iter()
        .filter(|declared| !declared.path.is_empty())
        .map(|declared| (declared.kind, declared.path))
        .collect()
}

struct ReleaseMatch {
    provenance: &'static str,
    version: Option<String>,
}

enum ProgramInspection {
    Ignored,
    Script,
    Software(HostSoftware),
}

/// The reporting pass over one host: the population rules and the
/// per-program readings. Population and output order stay outside this type;
/// its reads are independent so [`gather`] can bound and overlap them.
struct Reporter<'a> {
    target: &'a ComputeTarget,
    runner: &'a Runner,
    home: String,
    releases: String,
    releases_present: bool,
    hasher: Option<Hasher>,
    version_queries: BTreeMap<String, VersionQuery>,
}

/// Ask TARGET what it runs, natively: the population rules and per-program
/// readings of the retired reporter over the same audited channel
/// `host provenance` uses, with a modest fixed number of independent reads in
/// flight and nothing installed on the host.
///
/// Three sources make up the population, and all three are needed:
///
///   1. every program in `$HOME/.stado/bin` — what Stado placed on this host;
///   2. every declared service unit's program — what this host actually runs,
///      which is not the same set: a unit can name a program nothing
///      installed;
///   3. every release-control product install path bound by the caller —
///      brama lives at `<install_root>/bin/brama` and appears in neither of
///      the above.
///
/// Rows keep the same public and stored observation contract. Read-only,
/// and strictly so: files are hashed, unit files are read, and only catalogued
/// programs are asked their declared version query. Nothing is written.
pub async fn gather(
    target: &ComputeTarget,
    programs: &[String],
    runner: &Runner,
) -> Result<(Vec<HostSoftware>, usize), DeployError> {
    let home = host_channel::remote_home(target, runner).await?;
    let bin = format!("{home}/.stado/bin");
    let releases = format!("{home}/.stado/releases");
    let releases_present =
        host_channel::remote_test(target, &format!("-d {}", shlex_quote(&releases)), runner)
            .await?;
    let reporter = Reporter {
        target,
        runner,
        releases,
        releases_present,
        hasher: Hasher::resolve(target, runner).await?,
        version_queries: version_queries(&home)?,
        home,
    };

    // Preserve source order while admitting each concrete path once. In
    // particular, an installed binary that is also named by a service unit is
    // one observation, while same-named files at different paths remain two.
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    if host_channel::remote_test(target, &format!("-d {}", shlex_quote(&bin)), runner).await? {
        let listed = host_channel::run_program(target, &["/bin/ls", &bin], runner).await?;
        if listed.ok() {
            for name in listed.stdout.lines().filter(|name| !name.is_empty()) {
                let path = format!("{bin}/{name}");
                if seen.insert(path.clone()) {
                    paths.push(path);
                }
            }
        }
    }
    for (kind, path) in declared_units(target) {
        if let Some(program) = reporter.unit_program(&kind, &path).await? {
            if seen.insert(program.clone()) {
                paths.push(program);
            }
        }
    }
    for program in programs {
        if !program.is_empty() && seen.insert(program.clone()) {
            paths.push(program.clone());
        }
    }

    let mut inspections: Vec<(usize, Result<ProgramInspection, DeployError>)> =
        stream::iter(paths.into_iter().enumerate())
            .map(|(index, path)| {
                let reporter = &reporter;
                async move { (index, reporter.inspect_program(&path).await) }
            })
            .buffer_unordered(INSPECTION_CONCURRENCY)
            .collect()
            .await;
    inspections.sort_by_key(|(index, _)| *index);

    let mut rows = Vec::new();
    let mut scripts = 0;
    for (_, inspection) in inspections {
        match inspection? {
            ProgramInspection::Ignored => {}
            ProgramInspection::Script => scripts += 1,
            ProgramInspection::Software(row) => rows.push(row),
        }
    }

    Ok((rows, scripts))
}
