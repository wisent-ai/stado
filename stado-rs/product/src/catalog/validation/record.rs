//! One product's record in the catalog, checked field by field. The caller
//! ([`super::validate`]) names every refusal here as the product's.

use super::super::{
    text,
    types::{
        list_field, DeclaredKeychain, Dependency, Family, Integration, ProductStatus, RecipeKind,
        SurfaceKind, Visibility, FEATURES,
    },
};
use super::{choice, public_status, repository, roadmap, service, strings, submit};
use crate::common::slug;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

/// One product's record, `id` read from it: every refusal in it is named by
/// the caller as the product's, so a field this checks without naming the
/// product itself (`description: expected a string`) still says whose it is.
/// The ids, integrations and dependencies seen are kept for the checks that
/// need every product.
pub(super) fn record<'a>(
    product: &'a Value,
    id: &'a str,
    ids: &mut HashSet<&'a str>,
    targets: &mut Vec<(&'a str, &'a str)>,
    dependencies: &mut Vec<(&'a str, String)>,
) -> Result<()> {
    slug(id)?;
    if !ids.insert(id) {
        bail!("duplicate product {id}");
    }
    text(product, "name")?;
    text(product, "owner_repository")
        .and_then(repository)
        .with_context(|| format!("{id}.owner_repository"))?;
    text(product, "description")?;
    let status = choice::<ProductStatus>(product, "status")?;
    choice::<Visibility>(product, "visibility")?;
    // The public OWNER/REPO the product's in-app GitHub star request
    // links to: the product's own repository when strangers can see it,
    // another public one when they cannot (a private repository answers
    // them with a 404). Nothing a recipe executes reads it, so a record
    // without one is still the catalog's: only its star request refuses,
    // named by `stado product documentation star-repositories`, which
    // also asks GitHub whether each one is public. One a record does
    // declare must be OWNER/REPO.
    if product.get("star_repository").is_some() {
        text(product, "star_repository")
            .and_then(repository)
            .with_context(|| format!("{id}.star_repository"))?;
    }
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
        text(surface, "repository")
            .and_then(repository)
            .with_context(|| format!("{id}.surfaces.{kind}.repository"))?;
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
        text(recipe, "repository")
            .and_then(repository)
            .with_context(|| format!("{id}.installations.{surface}.repository"))?;
        let kind_word = text(recipe, "kind")?;
        let kind = RecipeKind::parse(kind_word)
            .with_context(|| format!("{id}: unsupported installation kind {kind_word}"))?;
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
        if recipe.get("signing_keychain").is_some() {
            DeclaredKeychain::parse(text(recipe, "signing_keychain")?)
                .with_context(|| format!("{id}.{surface}"))?;
        }
        if recipe.get("signing_role").is_some() {
            let role = text(recipe, "signing_role").with_context(|| format!("{id}.{surface}"))?;
            if role.contains('#') || recipe.get("signing_keychain").is_none() {
                bail!(
                    "{id}.{surface}: signing_role names a role (no #field) and is kept in the \
                         recipe's signing_keychain, which must be declared beside it"
                );
            }
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
        if let Some(health) = recipe.get("health") {
            let words = health.as_array().with_context(|| {
                format!(
                    "{id}.{surface}.health is {health}, not an argv array, so product sync \
                         cannot run it after an update; write it as [\"<binary>\", \"--version\"]"
                )
            })?;
            if words.is_empty()
                || words
                    .iter()
                    .any(|word| word.as_str().is_none_or(str::is_empty))
            {
                bail!(
                    "{id}.{surface}.health holds an empty word, so its command cannot be run; \
                         name one of the installation's binaries (or stado) and its arguments"
                );
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
    roadmap::validate(id, product)?;
    super::super::rivals::validate(id, product)?;
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
        let former: Vec<&str> = match product.get("former_names").and_then(Value::as_array) {
            Some(rows) => rows.iter().filter_map(|row| row["id"].as_str()).collect(),
            None => Vec::new(),
        };
        service::validate(id, service, &kinds, &former)?;
    }
    if let Some(checks) = product.get("public_status") {
        public_status::validate(id, checks)?;
    }
    submit::validate(id, product)?;
    Ok(())
}
