//! `stado product dependencies`: every call a product's source makes to
//! another product of the catalog, read from the code, against what the
//! catalog says the product does without it.
//!
//! The rule it holds the catalog to: a product runs without every other
//! product, falling to a popular solution when ours is absent. A product
//! whose code reaches another, where the catalog declares no dependency, a
//! dependency with no alternative, or an alternative with no code behind it,
//! is refused by name with the line that reaches it.

mod channel;
mod report;
mod scan;
mod standing;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use crate::common::Runtime;
use channel::{Channel, Names};
use scan::Site;
use standing::Standing;

/// One repository of a product, and whether its checkout was read.
#[derive(Serialize)]
pub(super) struct Repository {
    repository: String,
    checkout: String,
    read: bool,
}

/// One product the source reaches.
#[derive(Serialize)]
pub(super) struct Found {
    product: String,
    #[serde(flatten)]
    standing: Standing,
    channels: BTreeMap<Channel, usize>,
    sites: Vec<Site>,
}

/// One product's dependencies, as read from its repositories.
#[derive(Serialize)]
pub(super) struct Report {
    product: String,
    independent: bool,
    repositories: Vec<Repository>,
    dependencies: Vec<Found>,
}

/// The programs a product's installations declare, or its id when none does.
fn programs(product: &Value, id: &str) -> Vec<String> {
    let declared: BTreeSet<String> = product["installations"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|installation| installation["binaries"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if declared.is_empty() {
        return vec![id.to_owned()];
    }
    declared.into_iter().collect()
}

/// Every repository a product's record names: its owner and its surfaces.
fn repositories(product: &Value) -> BTreeSet<String> {
    let surfaces = product["surfaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|surface| surface["repository"].as_str());
    product["owner_repository"]
        .as_str()
        .into_iter()
        .chain(surfaces)
        .map(str::to_owned)
        .collect()
}

/// How many of `sites` reach through each channel.
fn per_channel(sites: &[Site]) -> BTreeMap<Channel, usize> {
    let mut grouped: BTreeMap<Channel, Vec<&Site>> = BTreeMap::new();
    for site in sites {
        grouped.entry(site.channel).or_default().push(site);
    }
    grouped
        .into_iter()
        .map(|(channel, sites)| (channel, sites.len()))
        .collect()
}

fn examine(runtime: &Runtime, product: &Value, everyone: &[Names]) -> Result<Report> {
    let id = crate::catalog::text(product, "id")?;
    let others: Vec<&Names> = everyone.iter().filter(|names| names.id != id).collect();
    let mut read = Vec::new();
    let mut sites: BTreeMap<String, Vec<Site>> = BTreeMap::new();
    for repository in repositories(product) {
        let name = repository
            .rsplit('/')
            .next()
            .with_context(|| format!("{id}: repository {repository:?} names no checkout"))?;
        let checkout = runtime.workspace.join(name);
        let present = checkout.join(".git").exists();
        if present {
            for (other, found) in scan::sites(&repository, &checkout, &others)? {
                sites.entry(other).or_default().extend(found);
            }
        }
        read.push(Repository {
            repository,
            checkout: checkout.display().to_string(),
            read: present,
        });
    }
    let declared: Vec<Value> = product["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    let mut dependencies = Vec::new();
    for (other, found) in sites {
        dependencies.push(Found {
            standing: standing::judge(&runtime.workspace, &declared, &other)?,
            channels: per_channel(&found),
            product: other,
            sites: found,
        });
    }
    let independent = read.iter().all(|repository| repository.read)
        && dependencies
            .iter()
            .all(|found| found.standing.independent());
    Ok(Report {
        product: id.to_owned(),
        independent,
        repositories: read,
        dependencies,
    })
}

/// Run `stado product dependencies [PRODUCT] [--json]`: the exit status is a
/// failure when any examined product cannot be shown to run without another.
pub fn run(arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let catalog = crate::catalog::current(runtime)?;
    let products = catalog["products"]
        .as_array()
        .context("catalog products must be a list")?;
    let mut everyone = Vec::new();
    for product in products {
        let id = crate::catalog::text(product, "id")?;
        everyone.push(Names::new(id, programs(product, id)));
    }
    let selected = arguments.get_one::<String>("positional");
    if let Some(wanted) = selected {
        if !everyone.iter().any(|names| &names.id == wanted) {
            bail!(
                "the catalog at {} names no product {wanted}",
                runtime.catalog.display()
            );
        }
    }
    let mut reports = Vec::new();
    for product in products {
        let id = crate::catalog::text(product, "id")?;
        if selected.is_some_and(|wanted| wanted != id) {
            continue;
        }
        reports.push(examine(runtime, product, &everyone)?);
    }
    report::print(&reports, arguments.get_flag("json"))?;
    let succeeded = reports.iter().all(|report| report.independent);
    Ok(i32::from(!succeeded))
}
