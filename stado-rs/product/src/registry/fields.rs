use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

pub fn surface(value: &str) -> Result<Value> {
    let (kind, payload) = value
        .split_once('=')
        .context("surface requires KIND=OWNER/REPO")?;
    let mut parts = payload.split(';');
    let repository = parts.next().context("surface repository is required")?;
    if kind.is_empty() || repository.is_empty() {
        bail!("surface requires KIND=OWNER/REPO");
    }
    let mut surface = json!({"kind": kind, "repository": repository});
    for part in parts {
        let (field, value) = part
            .split_once('=')
            .context("surface property requires FIELD=VALUE")?;
        if field != "onboarding_id" && field != "docs_origin" {
            bail!("unknown surface field {field}");
        }
        if value.is_empty() || surface.get(field).is_some() {
            bail!("empty or duplicate surface field {field}");
        }
        surface[field] = json!(value);
    }
    Ok(surface)
}

pub fn integration(value: &str) -> Result<Value> {
    let (product, rest) = value
        .split_once('=')
        .context("integration requires PRODUCT=SOURCE=DESCRIPTION")?;
    let (source, description) = rest
        .split_once('=')
        .context("integration requires PRODUCT=SOURCE=DESCRIPTION")?;
    if product.is_empty() || source.is_empty() || description.is_empty() {
        bail!("integration requires non-empty PRODUCT=SOURCE=DESCRIPTION");
    }
    Ok(json!({"product": product, "description": description, "source": source}))
}

pub fn installation(value: &str, surfaces: &Value) -> Result<Value> {
    let (surface, recipe) = value
        .split_once('=')
        .context("installation requires SURFACE=KIND[:PAYLOAD]")?;
    let (kind, payload) = recipe.split_once(':').unwrap_or((recipe, ""));
    let repository = surfaces
        .as_array()
        .context("surfaces must be a list")?
        .iter()
        .find(|row| row["kind"] == surface)
        .context("installation names an undeclared surface")?["repository"]
        .clone();
    let primary = match kind {
        "stado-release" | "desktop-release" | "cargo" => "manifest",
        "local-build" => "command",
        "npm" | "pipx" => "binaries",
        "pip" => "package",
        _ => bail!("unknown installation kind {kind}"),
    };
    let mut recipe = json!({"surface": surface, "kind": kind, "repository": repository});
    if payload.is_empty() {
        return Ok(recipe);
    }
    let explicit = match kind {
        "cargo" => {
            payload.starts_with("manifest=")
                || payload.starts_with("binaries=")
                || payload.starts_with("features=")
        }
        "pip" => payload.starts_with("package=") || payload.starts_with("entrypoints="),
        _ => false,
    };
    let parts = if explicit {
        payload
            .split(';')
            .filter(|part| !part.is_empty())
            .map(|part| {
                part.split_once('=')
                    .context("recipe property requires FIELD=VALUE")
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        vec![(primary, payload)]
    };
    for (field, content) in parts {
        let allowed = match kind {
            "cargo" => matches!(field, "manifest" | "binaries" | "features"),
            "pip" => matches!(field, "package" | "entrypoints"),
            _ => field == primary,
        };
        if !allowed || content.is_empty() || recipe.get(field).is_some() {
            bail!("invalid, empty or duplicate {kind} recipe field {field}");
        }
        recipe[field] = match field {
            "binaries" | "entrypoints" | "features" => json!(content
                .split(',')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()),
            _ => json!(content),
        };
    }
    Ok(recipe)
}
