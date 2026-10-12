use super::{
    text,
    types::{
        list_field, Dependency, Family, Integration, ProductStatus, RecipeKind, RoadmapItem,
        SurfaceKind, Visibility, FEATURES,
    },
};
use crate::common::slug;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

mod service;

/// The field parses as `T`, or the refusal names the field and the word.
fn choice<'a, T: Deserialize<'a>>(value: &'a Value, field: &str) -> Result<T> {
    T::deserialize(&value[field])
        .with_context(|| format!("{field}: {} is not one of its values", value[field]))
}

pub fn validate(document: &Value) -> Result<()> {
    if document["schema_version"] != 1 {
        bail!("catalog schema_version must be 1");
    }
    let products = document["products"]
        .as_array()
        .context("products must be a list")?;
    let mut ids = HashSet::new();
    let mut targets = Vec::new();
    let mut dependencies = Vec::new();
    for product in products {
        let id = text(product, "id")?;
        slug(id)?;
        if !ids.insert(id) {
            bail!("duplicate product {id}");
        }
        text(product, "name")?;
        repository(text(product, "owner_repository")?)?;
        text(product, "description")?;
        let status = choice::<ProductStatus>(product, "status")?;
        choice::<Visibility>(product, "visibility")?;
        choice::<Family>(product, "family")?;
        strings(product, "evidence")?;
        for field in ["approved_by", "approved_at", "approval_note"] {
            if product.get(field).is_some() {
                text(product, field)?;
            }
        }
        if product.get("approved_by").is_some() {
            text(product, "approved_at")?;
        }
        if product.get("approval_note").is_some() {
            text(product, "approved_by")?;
        }
        let surfaces = product["surfaces"]
            .as_array()
            .context("surfaces must be a list")?;
        if surfaces.is_empty() {
            bail!("{id}.surfaces: at least one surface is required");
        }
        let mut kinds = HashSet::new();
        for surface in surfaces {
            let kind = text(surface, "kind")?;
            if !kinds.insert(kind) {
                bail!("{id}: duplicate {kind} surface");
            }
            repository(text(surface, "repository")?)?;
            if let Some(origin) = surface.get("docs_origin") {
                let origin = origin
                    .as_str()
                    .context("docs_origin must be an HTTPS origin")?;
                let url = url::Url::parse(origin)?;
                if kind != "cli"
                    || url.scheme() != "https"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.path() != "/"
                    || url.query().is_some()
                    || url.fragment().is_some()
                    || origin.chars().any(char::is_whitespace)
                {
                    bail!("{id}.docs_origin: expected an HTTPS origin for a CLI surface");
                }
            }
        }
        let empty = Vec::new();
        let installations = match product.get("installations") {
            None if status == ProductStatus::Preview => &empty,
            _ => product["installations"]
                .as_array()
                .context("installations must be a list")?,
        };
        if installations.is_empty() && status != ProductStatus::Preview {
            bail!("{id}: an active or retired product requires installation recipes");
        }
        let mut installed = HashSet::new();
        for recipe in installations {
            let surface = text(recipe, "surface")?;
            if !kinds.contains(surface) || !installed.insert(surface) {
                bail!("{id}: undeclared or duplicate installation surface {surface}");
            }
            repository(text(recipe, "repository")?)?;
            let kind_word = text(recipe, "kind")?;
            let Some(kind) = RecipeKind::parse(kind_word) else {
                bail!("{id}: unsupported installation kind {kind_word}");
            };
            for field in std::iter::once(kind.primary()).chain(kind.also_required()) {
                if list_field(field) {
                    strings(recipe, field)?;
                } else {
                    text(recipe, field)?;
                }
            }
            if recipe.get(FEATURES).is_some() {
                if !kind.allows(FEATURES) {
                    bail!("features only apply to cargo recipes");
                }
                strings(recipe, FEATURES)?;
            }
            if let Some(commands) = recipe.get("after_install") {
                for command in commands
                    .as_array()
                    .context("after_install must be an array")?
                {
                    let words = command
                        .as_array()
                        .context("after_install commands must be argv arrays")?;
                    if words.is_empty()
                        || words
                            .iter()
                            .any(|word| word.as_str().is_none_or(str::is_empty))
                    {
                        bail!("after_install commands require non-empty string arguments");
                    }
                }
            }
        }
        if status != ProductStatus::Preview {
            for surface in surfaces {
                if SurfaceKind::deserialize(&surface["kind"])?.needs_recipe()
                    && !installed.contains(text(surface, "kind")?)
                {
                    bail!("{id}: no installation for {}", surface["kind"]);
                }
            }
        }
        for item in product["roadmap"]
            .as_array()
            .context("roadmap must be a list")?
        {
            // The status is a RoadmapStatus, so an unknown one is refused here
            // with serde's reason and the item it came from.
            let row = RoadmapItem::deserialize(item)
                .with_context(|| format!("{id}: invalid roadmap item"))?;
            if row.title.trim().is_empty()
                || row.outcome.trim().is_empty()
                || row.source.trim().is_empty()
            {
                bail!("{id}: roadmap title, outcome and source must be non-empty");
            }
        }
        super::rivals::validate(id, product)?;
        for item in product["integrations"]
            .as_array()
            .context("integrations must be a list")?
        {
            let integration = Integration::deserialize(item)?;
            if integration.product == id {
                bail!("{id}: self-integration is not allowed");
            }
            if integration.description.trim().is_empty() || integration.source.trim().is_empty() {
                bail!("{id}: integration description and source must be non-empty");
            }
            targets.push((id, integration.product));
        }
        if let Some(declared) = product.get("dependencies") {
            for item in declared.as_array().context("dependencies must be a list")? {
                let dependency = Dependency::deserialize(item)
                    .with_context(|| format!("{id}: invalid dependency"))?;
                if dependency.alternative_source.is_some() && dependency.alternative.is_none() {
                    bail!(
                        "{id}: the dependency on {} names alternative_source without the \
                         alternative it implements",
                        dependency.product
                    );
                }
                if [&dependency.alternative, &dependency.alternative_source]
                    .into_iter()
                    .flatten()
                    .any(|text| text.trim().is_empty())
                {
                    bail!(
                        "{id}: the dependency on {} has an empty alternative",
                        dependency.product
                    );
                }
                if let Some(because) = &dependency.bound_because {
                    if dependency.alternative.is_some() {
                        bail!(
                            "{id}: the dependency on {} declares bound_because beside an \
                             alternative; a need is bound by design or has a way around it, \
                             not both",
                            dependency.product
                        );
                    }
                    if because.trim().is_empty() {
                        bail!(
                            "{id}: the dependency on {} has an empty bound_because",
                            dependency.product
                        );
                    }
                }
                dependencies.push((id, dependency.product));
            }
        }
        if let Some(service) = product.get("service") {
            service::validate(id, service, &kinds)?;
        }
    }
    for (id, target) in targets {
        if !ids.contains(target) {
            bail!("{id}: integration names unknown product {target}");
        }
    }
    for (id, target) in dependencies {
        if !ids.contains(target.as_str()) {
            bail!("{id}: dependency names unknown product {target}");
        }
    }
    Ok(())
}

fn strings(value: &Value, field: &str) -> Result<()> {
    let values = value[field]
        .as_array()
        .with_context(|| format!("{field} must be a list"))?;
    if values.is_empty()
        || values
            .iter()
            .any(|v| v.as_str().is_none_or(|s| s.trim().is_empty()))
    {
        bail!("{field} requires non-empty strings");
    }
    Ok(())
}

fn repository(value: &str) -> Result<()> {
    let parts: Vec<_> = value.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
    {
        bail!("invalid repository {value:?}; expected OWNER/REPO");
    }
    Ok(())
}
