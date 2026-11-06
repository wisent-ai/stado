//! The product-independence judgement over the lines one revision adds: what
//! `stado quality check` refuses before a push, a change hand-off or a
//! delivery lands coupling the catalog cannot show the product running
//! without.
//!
//! `stado product dependencies` reads every line of every checkout, so a
//! product whose base already reaches another without a declared alternative
//! would refuse every later push of it for a line nobody touched. Here only
//! the lines the revision adds are read, and each product they reach is
//! judged exactly as the whole-catalog reading judges it: undeclared, bound,
//! an alternative with no code named, or code named that the workspace does
//! not hold. The product the catalog names as its authority (`authority.owner`)
//! installs and manages every product, so naming them is its work; it is not
//! judged here, and `stado product dependencies` still reports it.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

use super::channel::Names;
use super::scan::{self, Site};
use super::standing;
use crate::catalog::committed::{Choice, Provenance};
use crate::common::Runtime;

pub use scan::Added;

/// One product a revision's added lines reach without the catalog showing
/// the revision's product running without it, with what to do about it.
fn refused_line(product: &str, other: &str, reason: &str, sites: &[Site]) -> String {
    let lines: Vec<String> = sites
        .iter()
        .map(|site| format!("{}:{}", site.file, site.line))
        .collect();
    format!(
        "{product} cannot be shown to run without {other}: {reason}; added at {}. Reach {other} \
         through `stado capabilities` when a family offers it, or declare what {product} does \
         without it: `stado product registry set {product} --add-dependency \
         '{{\"product\":\"{other}\",\"surface\":\"<surface>\",\"for_surface\":\"<surface>\",\
         \"alternative\":\"<what {product} does without it>\",\"alternative_source\":\"<file in \
         the workspace that does it>\"}}'`",
        lines.join(", ")
    )
}

/// What the lines one revision adds say about its product running alone.
pub enum Verdict {
    /// Every catalog product they reach is one the catalog shows the
    /// product running without, or they reach none.
    Independent,
    /// They are not judged, and why: the product is the catalog's authority,
    /// or a product the catalog does not record (a fixture, a product not
    /// yet catalogued), which declares nothing to judge against.
    Unjudged(String),
    /// One sentence per product reached that the catalog does not show the
    /// product running without, naming the file and line of every site and
    /// what to declare.
    Refused(String),
}

/// The catalog a verdict was judged against, as the verdict names it. A
/// tracked catalog is read as every catalog reader reads it: at its
/// checkout's `origin/main` when the committed head is behind it, at the
/// committed head otherwise (one ahead of origin is read with what it adds);
/// an untracked file as it stands; none at all as the catalog this build
/// embeds.
fn read_from(runtime: &Runtime, provenance: Option<&Provenance>) -> String {
    let chosen = |choice: Choice| match choice {
        Choice::PublishedMain => "its origin/main, which the checkout's head is behind",
        Choice::CommittedHead => "its committed head",
        Choice::InstalledRevision => "the revision being installed",
        Choice::Embedded => "the catalog this build embeds",
    };
    match provenance {
        Some(read) => format!(
            "the catalog {} at {} ({})",
            read.source,
            read.revision,
            chosen(read.chosen)
        ),
        None if runtime.embedded_catalog => "the catalog this build embeds".to_string(),
        None => format!("the catalog {} as it stands", runtime.catalog.display()),
    }
}

/// What the lines `added` adds to `product`, exported at `tree`, say about
/// it running without every other catalog product ([`Verdict`]). The release
/// manifests its installations declare are not read as code it runs.
pub fn judge(runtime: &Runtime, product: &str, tree: &Path, added: &[Added]) -> Result<Verdict> {
    let (catalog, provenance) = crate::catalog::read(runtime)?;
    let read = read_from(runtime, provenance.as_ref());
    if catalog["authority"]["owner"] == product {
        return Ok(Verdict::Unjudged(format!(
            "{product} is the catalog's authority by {read}, and installs and manages every \
             product; `stado product dependencies {product}` reports what it reaches"
        )));
    }
    let products = catalog["products"]
        .as_array()
        .context("catalog products must be a list")?;
    let Some(record) = products.iter().find(|record| record["id"] == product) else {
        return Ok(Verdict::Unjudged(format!(
            "{read} names no product {product}, so it declares nothing the lines can be judged \
             against; `stado product registry` adds its record"
        )));
    };
    let mut everyone = Vec::new();
    for other in products {
        let id = crate::catalog::text(other, "id")?;
        everyone.push(Names::new(id, super::programs(other, id)));
    }
    let others: Vec<&Names> = everyone
        .iter()
        .filter(|names| names.id != product)
        .collect();
    let manifests: Vec<String> = super::repositories(record)
        .iter()
        .flat_map(|repository| super::installation_manifests(record, repository))
        .collect();
    let repository = crate::catalog::text(record, "owner_repository")?;
    let reached = scan::added_sites(repository, tree, &manifests, added, &others)?;
    let declared: Vec<Value> = record["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    let mut said = Vec::new();
    for (other, sites) in &reached {
        let judged = standing::judge(&runtime.workspace, &declared, other)?;
        if let Some(reason) = judged.refusal(product, other) {
            said.push(refused_line(product, other, &reason, sites));
        }
    }
    Ok(match said.is_empty() {
        true => Verdict::Independent,
        false => Verdict::Refused(format!("{}\n(judged against {read})", said.join("\n"))),
    })
}
