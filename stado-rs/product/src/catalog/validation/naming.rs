//! A product's names held to their rules: its id, its display name, the
//! decision that gave them (`naming`) and every name it had before
//! (`former_names`).
//!
//! A name, current or former, belongs to one product. A former name stays
//! with the product that had it, so a name read in an older transcript,
//! commit or page still finds exactly the product it meant
//! (`stado product name show NAME`); giving it to another product would make
//! that lookup ambiguous. Names are compared without regard to case or the
//! spaces around them, because a reader does not tell `Echo` from `echo`.

use super::super::text;
use crate::{common::slug, name::key};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// The decision that gave a product its current names.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    decided_by: String,
    decided_at: String,
    reason: String,
}

/// One name a product had before, with the decision it stood under when
/// that was recorded, and when it stopped being the product's name.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FormerName {
    id: String,
    name: String,
    until: String,
    #[serde(default)]
    decided_by: Option<String>,
    #[serde(default)]
    decided_at: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// Who holds each name, keyed by the name as it is compared.
type Holders<'a> = BTreeMap<String, (&'a str, &'static str)>;

/// Every product's names and naming records.
pub(super) fn validate(products: &[Value]) -> Result<()> {
    let mut holders = Holders::new();
    for product in products {
        let id = text(product, "id")?;
        claim(&mut holders, id, id, "id")?;
        claim(&mut holders, text(product, "name")?, id, "display name")?;
        if let Some(decision) = product.get("naming") {
            let decision = Decision::deserialize(decision).with_context(|| {
                format!("{id}.naming: expected decided_by, decided_at and reason, and nothing else")
            })?;
            for (field, value) in [
                ("decided_by", &decision.decided_by),
                ("reason", &decision.reason),
            ] {
                if value.trim().is_empty() {
                    bail!("{id}.naming.{field} is empty; a naming decision names who decided and why");
                }
            }
            past(id, "naming.decided_at", &decision.decided_at)?;
        }
        let Some(former) = product.get("former_names") else {
            continue;
        };
        let rows = former
            .as_array()
            .with_context(|| format!("{id}.former_names must be a list"))?;
        for row in rows {
            let former = FormerName::deserialize(row).with_context(|| {
                format!(
                    "{id}.former_names: each row is id, name and until, with the decided_by, \
                     decided_at and reason it stood under when they were recorded"
                )
            })?;
            slug(&former.id).with_context(|| format!("{id}.former_names"))?;
            if former.name.trim().is_empty() {
                bail!("{id}.former_names: the row for {} has an empty name", former.id);
            }
            past(id, "former_names.until", &former.until)?;
            if let Some(at) = &former.decided_at {
                past(id, "former_names.decided_at", at)?;
            }
            for (field, value) in [("decided_by", &former.decided_by), ("reason", &former.reason)] {
                if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
                    bail!("{id}.former_names: the row for {} has an empty {field}", former.id);
                }
            }
        }
        // Claimed after the rows were read whole, from the record itself, so
        // the holders borrow the catalog document rather than the parsed rows.
        for row in rows {
            claim(&mut holders, text(row, "id")?, id, "former id")?;
            claim(&mut holders, text(row, "name")?, id, "former display name")?;
        }
    }
    Ok(())
}

/// `word` is held by product `owner` as `what`; another product holding it
/// already is refused, naming that product and how it holds the name.
fn claim<'a>(
    holders: &mut Holders<'a>,
    word: &'a str,
    owner: &'a str,
    what: &'static str,
) -> Result<()> {
    let compared = key(word);
    if let Some((other, held_as)) = holders.get(&compared) {
        if *other != owner {
            bail!(
                "{owner}: its {what} {word:?} is already the {held_as} of {other}; a name, current \
                 or former, belongs to one product, so give {owner} another name \
                 (`stado product name check NAME` says whether a name is free)"
            );
        }
        return Ok(());
    }
    holders.insert(compared, (owner, what));
    Ok(())
}

/// `at` is an RFC 3339 time that is not in the future.
fn past(id: &str, field: &str, at: &str) -> Result<()> {
    let parsed = chrono::DateTime::parse_from_rfc3339(at)
        .with_context(|| format!("{id}.{field}: {at} is not an RFC 3339 time"))?;
    if parsed > chrono::Utc::now() {
        bail!("{id}.{field}: {at} is in the future; a name is decided before it is recorded");
    }
    Ok(())
}
