use super::{
    super::{destination, ownership, remove_path, sync_tree},
    previous,
};
use crate::{
    catalog::text,
    common::{lock, now, Runtime},
    install::{plan::Placement, services},
    state::{self, ProductState},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn rollback(runtime: &Runtime, product: &Value, surface: &str) -> Result<ProductState> {
    let id = text(product, "id")?;
    let _writer = lock(&state::path(runtime, id, surface)?.with_extension("lock"))?;
    let _ownership = ownership::writer(runtime)?;
    let mut current = ProductState::load(runtime, id, surface)?
        .context("product surface has no recorded installation")?;
    if (current.status == "rolled_back"
        || (current.status == "absent" && current.previous.is_none()))
        && current.extra.contains_key("rollback")
    {
        return Ok(current);
    }
    if current.backups.is_empty()
        && current.installed_paths.is_empty()
        && current.previous.is_none()
    {
        bail!("{id} {surface} has no retained installation to roll back");
    }
    let shared = ownership::shared(runtime, id, surface)?;
    let root = runtime
        .home
        .join(".stado/products")
        .join(id)
        .join("backups");
    let mut restored = BTreeMap::new();
    for saved in &current.backups {
        destination(runtime, &saved.path)?;
        if !saved.backup.starts_with(&root) {
            bail!(
                "rollback backup is outside the product's owned history: {}",
                saved.backup.display()
            );
        }
        let parent = saved
            .backup
            .parent()
            .context("rollback backup has no parent")?
            .canonicalize()?;
        if !parent.starts_with(root.canonicalize()?) {
            bail!("rollback backup parent escapes owned history");
        }
        let fingerprint = Placement {
            source: saved.backup.clone(),
            destination: saved.path.clone(),
            symbolic: false,
        }
        .fingerprint()?;
        let expected = current
            .extra
            .get("backup_fingerprints")
            .and_then(|values| values.get(saved.backup.to_string_lossy().as_ref()))
            .with_context(|| {
                format!(
                    "retained backup has no byte evidence: {}",
                    saved.backup.display()
                )
            })?;
        if expected != &fingerprint {
            bail!("rollback backup changed: {}", saved.backup.display());
        }
        let actual = ownership::guard(&current, &saved.path)?;
        if ownership::overlaps(&saved.path, &shared) && actual.as_ref() != Some(&fingerprint) {
            bail!(
                "rollback would change a path another installed surface still owns: {}",
                saved.path.display()
            );
        }
        restored.insert(saved.path.clone(), fingerprint);
    }
    for path in &current.installed_paths {
        if !ownership::overlaps(path, &shared) {
            ownership::guard(&current, path)?;
        }
    }
    let mut previous = previous::observed(&current, &restored, &shared)?;
    if surface == "service" && previous.status == "absent" {
        bail!("the service receipt has no predecessor; remote absence must be established by the managed service lifecycle before rollback");
    }
    current.status = "rolling_back".to_owned();
    current.save(runtime)?;
    for saved in &current.backups {
        if ownership::overlaps(&saved.path, &shared) {
            continue;
        }
        ownership::guard(&current, &saved.path)?;
        Placement {
            source: saved.backup.clone(),
            destination: saved.path.clone(),
            symbolic: false,
        }
        .place()?;
        sync_tree(&saved.path)?;
    }
    for path in &current.installed_paths {
        if !restored.contains_key(path) && !ownership::overlaps(path, &shared) {
            ownership::guard(&current, path)?;
            remove_path(runtime, path)?;
        }
    }
    ownership::verify(&previous)?;
    if surface == "service" {
        let host = previous
            .host
            .as_deref()
            .context("rollback service receipt has no host")?;
        previous.extra.insert(
            "service".to_owned(),
            services::ensure(product, &previous.recipe, host)?,
        );
    }
    if previous.status != "absent" {
        previous.status = "rolled_back".to_owned();
    }
    previous.installed_at = now();
    previous.extra.insert("rollback".to_owned(), json!({"method": "exact-backup", "restored_from": current.installed_at, "retained": current.backups}));
    previous.save(runtime)?;
    Ok(previous)
}
