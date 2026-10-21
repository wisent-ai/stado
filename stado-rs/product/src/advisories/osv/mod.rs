//! The advisory database: the crates.io advisories an OSV database publishes
//! (https://ossf.github.io/osv-schema/), read whole from its ecosystem
//! archive and evaluated here against each locked version.
//!
//! The archive is read rather than asked per package: one download answers
//! every lockfile of the workspace, so neither the number of packages nor a
//! service's batch limit decides whether the answer is complete.

mod range;

use anyhow::{Context, Result};
use range::{evaluate, Event};
use semver::{Version, VersionReq};
use serde::Deserialize;
use stado_wait as wait;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};
use url::Url;

/// The public OSV database, whose storage serves each ecosystem's advisories
/// as `<origin>/<ecosystem>/all.zip` (https://google.github.io/osv.dev/data/).
pub const PUBLIC_DATABASE: &str = "https://osv-vulnerabilities.storage.googleapis.com";

/// The ecosystem a Cargo lockfile's crates.io packages belong to, as OSV
/// names it.
pub const ECOSYSTEM: &str = "crates.io";

/// The archive of every advisory of one ecosystem, in the database's layout.
const ARCHIVE: &str = "all.zip";

#[derive(Deserialize)]
struct Record {
    id: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    withdrawn: Option<String>,
    #[serde(default)]
    affected: Vec<Affected>,
}

#[derive(Deserialize)]
struct Affected {
    package: Option<PackageName>,
    #[serde(default)]
    ranges: Vec<Range>,
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    database_specific: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct PackageName {
    ecosystem: String,
    name: String,
}

/// A range of versions. A range that names a repository counts commits of
/// it rather than versions (OSV's `GIT` type) and says nothing about a
/// locked version.
#[derive(Deserialize)]
struct Range {
    repo: Option<String>,
    #[serde(default)]
    events: Vec<Event>,
}

/// One advisory that affects one locked version.
#[derive(Clone, Debug)]
pub struct Hit {
    pub advisory: String,
    pub aliases: Vec<String>,
    pub summary: Option<String>,
    /// RustSec's `informational` class (`unmaintained`, `unsound`, `notice`)
    /// when the advisory is a notice rather than a vulnerability.
    pub informational: Option<String>,
    /// Every version that closes an affected range, in version order.
    pub fixed: Vec<String>,
    /// The first fixed version Cargo may move this lock to without a manifest
    /// change: above the locked one and caret-compatible with it.
    pub compatible_fix: Option<String>,
}

/// The crates.io advisories of one database, and for each package name the
/// advisories that name it.
pub struct Database {
    pub origin: String,
    records: Vec<Record>,
    by_package: BTreeMap<String, Vec<usize>>,
}

impl Database {
    /// Download and read the database's crates.io archive.
    pub fn read(origin: &str) -> Result<Self> {
        let mut url = Url::parse(origin)
            .with_context(|| format!("--database {origin:?} is not a URL; give an https origin"))?;
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("--database {origin} cannot carry a path"))?
            .pop_if_empty()
            .extend([ECOSYSTEM, ARCHIVE]);
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!(
                "stado-product-advisories/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .context("building the HTTP client for the advisory database")?;
        let response = wait::request_blocking(client.get(url.clone())).map_err(|error| {
            anyhow::anyhow!(
                "the advisory database {origin} did not answer {url}: {}",
                wait::chain(&error)
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!(
                "the advisory database {origin} answered {status} to {url}; an OSV database \
                 serves the {ECOSYSTEM} advisories there"
            );
        }
        let bytes = response
            .bytes()
            .with_context(|| format!("reading {url} from the advisory database {origin}"))?;
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).with_context(|| {
            format!("{url}: the advisory database {origin} answered with no zip archive")
        })?;
        let mut records = Vec::new();
        for index in 0..archive.len() {
            let mut member = archive
                .by_index(index)
                .with_context(|| format!("{url}: reading archive member {index}"))?;
            if member.is_dir() {
                continue;
            }
            let name = member.name().to_owned();
            let mut text = String::new();
            member
                .read_to_string(&mut text)
                .with_context(|| format!("{url}: reading {name}"))?;
            let record: Record = serde_json::from_str(&text)
                .with_context(|| format!("{url}: {name} is not an OSV advisory"))?;
            if record.withdrawn.is_none() {
                records.push(record);
            }
        }
        let mut by_package: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, record) in records.iter().enumerate() {
            let names: BTreeSet<&str> = record
                .affected
                .iter()
                .filter_map(|affected| affected.package.as_ref())
                .filter(|package| package.ecosystem == ECOSYSTEM)
                .map(|package| package.name.as_str())
                .collect();
            for name in names {
                by_package.entry(name.to_owned()).or_default().push(index);
            }
        }
        Ok(Self {
            origin: origin.to_owned(),
            records,
            by_package,
        })
    }

    /// Whether the database holds an advisory with this id or alias.
    pub fn knows(&self, id: &str) -> bool {
        self.records
            .iter()
            .any(|record| record.id == id || record.aliases.iter().any(|alias| alias == id))
    }

    /// Every advisory that affects `name` at `version`, in advisory order.
    pub fn affecting(&self, name: &str, version: &str) -> Result<Vec<Hit>> {
        let locked = Version::parse(version).with_context(|| {
            format!("{name} is locked at {version}, which is not a semver version")
        })?;
        let caret = VersionReq::parse(&format!("^{locked}"))
            .with_context(|| format!("{name} {version} has no caret requirement"))?;
        let mut hits = Vec::new();
        let indices = self.by_package.get(name).into_iter().flatten();
        for record in indices.map(|index| &self.records[*index]) {
            let named = record.affected.iter().filter(|affected| {
                affected
                    .package
                    .as_ref()
                    .is_some_and(|package| package.ecosystem == ECOSYSTEM && package.name == name)
            });
            for affected in named {
                let mut fixed = Vec::new();
                let mut hit = affected.versions.iter().any(|listed| listed == version);
                for range in affected.ranges.iter().filter(|range| range.repo.is_none()) {
                    let (inside, closes) = evaluate(&record.id, &range.events, &locked)?;
                    hit |= inside;
                    fixed.extend(closes);
                }
                if !hit {
                    continue;
                }
                fixed.sort();
                fixed.dedup();
                let compatible_fix = fixed
                    .iter()
                    .find(|fix| **fix > locked && caret.matches(fix))
                    .map(Version::to_string);
                hits.push(Hit {
                    advisory: record.id.clone(),
                    aliases: record.aliases.clone(),
                    summary: record.summary.clone(),
                    informational: affected
                        .database_specific
                        .as_ref()
                        .and_then(|specific| specific["informational"].as_str())
                        .map(str::to_owned),
                    fixed: fixed.iter().map(Version::to_string).collect(),
                    compatible_fix,
                });
            }
        }
        hits.sort_by(|left, right| left.advisory.cmp(&right.advisory));
        Ok(hits)
    }
}
