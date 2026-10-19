//! Ordered provider revisions and their decisions for an installation plan.

use super::requirements;
use crate::{catalog, common::Runtime};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;

struct Plan<'a> {
    runtime: &'a Runtime,
    document: &'a Value,
    without: &'a [String],
    seen: BTreeSet<String>,
    visiting: Vec<String>,
    rows: Vec<Value>,
}

impl Plan<'_> {
    fn append(&mut self, product: &Value, surface: &str, revision: &str) -> Result<()> {
        let id = catalog::text(product, "id")?;
        let key = format!("{id}:{surface}");
        if self.visiting.contains(&key) {
            bail!(
                "verb prerequisite cycle: {} -> {key}",
                self.visiting.join(" -> ")
            );
        }
        if !self.seen.insert(key.clone()) {
            return Ok(());
        }
        self.visiting.push(key);
        for decision in requirements(
            self.runtime,
            self.document,
            product,
            surface,
            revision,
            &[],
            self.without,
        )? {
            let provider = &decision.requirement.product;
            let document = self.document;
            let provided = catalog::product(document, provider)?;
            self.append(provided, &decision.requirement.surface, &decision.green)?;
            self.rows.push(decision.report());
        }
        self.visiting.pop();
        Ok(())
    }
}

pub fn chain(
    runtime: &Runtime,
    document: &Value,
    product: &Value,
    surface: &str,
    revision: &str,
    without: &[String],
) -> Result<Vec<Value>> {
    let mut plan = Plan {
        runtime,
        document,
        without,
        seen: BTreeSet::new(),
        visiting: Vec::new(),
        rows: Vec::new(),
    };
    plan.append(product, surface, revision)?;
    plan.rows.push(
        json!({"product": catalog::text(product, "id")?, "surface": surface,
                          "revision": revision, "action": "install"}),
    );
    Ok(plan.rows)
}
