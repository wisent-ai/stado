//! A product's roadmap as the catalog keeps it: every item names its title,
//! outcome and source, and an item that ended keeps its place with the
//! decision that ended it. A delivered, rejected or superseded item names
//! where that was decided (`decision`); a superseded one also names the item
//! of the same product that replaced it (`superseded_by`). An item still
//! planned or in progress carries neither, so a decision is never read off
//! an open item. Removing an item is for one recorded in error; an idea that
//! was dropped or replaced is recorded as such, so it reads back later.

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use super::super::types::{RoadmapItem, RoadmapStatus};

/// Every roadmap item of product `id`, refused naming the item and what it
/// lacks.
pub(super) fn validate(id: &str, product: &Value) -> Result<()> {
    let items = product["roadmap"]
        .as_array()
        .context("roadmap must be a list")?;
    // The status is a RoadmapStatus, so an unknown one is refused here with
    // serde's reason and the item it came from.
    let rows = items
        .iter()
        .map(|item| {
            RoadmapItem::deserialize(item)
                .with_context(|| format!("{id}: invalid roadmap item {item}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut titles = HashSet::new();
    for row in &rows {
        if row.title.trim().is_empty()
            || row.outcome.trim().is_empty()
            || row.source.trim().is_empty()
        {
            bail!("{id}: roadmap title, outcome and source must be non-empty");
        }
        if !titles.insert(row.title.trim()) {
            bail!(
                "{id}: roadmap item {:?} is listed twice; one title is one item",
                row.title
            );
        }
    }
    for row in &rows {
        let title = row.title;
        let decision = row
            .decision
            .map(str::trim)
            .filter(|decision| !decision.is_empty());
        match (row.status.decided(), decision) {
            (true, None) => bail!(
                "{id}: roadmap item {title:?} is {:?}: decision names where that was decided (the operator's message, the release or the repository line) and must be non-empty",
                row.status
            ),
            (false, Some(_)) => bail!(
                "{id}: roadmap item {title:?} is {:?} and carries a decision; decision belongs to a delivered, rejected or superseded item",
                row.status
            ),
            _ => {}
        }
        let replaced = row
            .superseded_by
            .map(str::trim)
            .filter(|named| !named.is_empty());
        match (row.status, replaced) {
            (RoadmapStatus::Superseded, None) => bail!(
                "{id}: roadmap item {title:?} is superseded: superseded_by names the item of {id} that replaced it"
            ),
            (RoadmapStatus::Superseded, Some(named)) if named == title.trim() => {
                bail!("{id}: roadmap item {title:?} cannot supersede itself")
            }
            (RoadmapStatus::Superseded, Some(named)) if !titles.contains(named) => bail!(
                "{id}: roadmap item {title:?} is superseded by {named:?}, which is no roadmap item of {id}; add that item first"
            ),
            (RoadmapStatus::Superseded, Some(_)) | (_, None) => {}
            (_, Some(named)) => bail!(
                "{id}: roadmap item {title:?} is {:?} and names {named:?} as superseded_by; only a superseded item names its replacement",
                row.status
            ),
        }
    }
    Ok(())
}
