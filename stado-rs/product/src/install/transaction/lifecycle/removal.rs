use super::super::{backup, backup_fingerprints, ownership, remove_path};
use crate::{
    catalog::text,
    common::{lock, now, Runtime},
    install::services,
    state::{self, ProductState},
};
use anyhow::{bail, Context, Result};
use serde_json::Value;

pub fn remove(
    runtime: &Runtime,
    product: &Value,
    surface: &str,
    host: Option<&str>,
) -> Result<ProductState> {
    let id = text(product, "id")?;
    let _writer = lock(&state::path(runtime, id, surface)?.with_extension("lock"))?;
    let _ownership = ownership::writer(runtime)?;
    let mut current = ProductState::load(runtime, id, surface)?
        .context("product surface has no recorded installation")?;
    if current.status == "absent" {
        return Ok(current);
    }
    if current.status == "installing" || current.status == "rolling_back" {
        bail!("finish or roll back the interrupted installation before removing it");
    }
    if surface == "service" && host.or(current.host.as_deref()).is_none() {
        bail!("service removal requires its recorded --host");
    }
    let shared = ownership::shared(runtime, id, surface)?;
    let removable: Vec<_> = current
        .installed_paths
        .iter()
        .filter(|path| !ownership::overlaps(path, &shared))
        .cloned()
        .collect();
    if current.status != "removing" {
        ownership::verify(&current)
            .context("recorded files changed; refusing to delete unverified replacement files")?;
        let previous = current.clone();
        current.backups = backup(runtime, id, &removable)?;
        current.extra.insert(
            "backup_fingerprints".to_owned(),
            backup_fingerprints(&current.backups)?,
        );
        current.previous = Some(Box::new(previous));
        current.status = "removing".to_owned();
        current.save(runtime)?;
    }
    if surface == "service" && current.extra.get("service_removed") != Some(&Value::Bool(true)) {
        services::remove(product, &current, host.or(current.host.as_deref()).unwrap())?;
        current
            .extra
            .insert("service_removed".to_owned(), Value::Bool(true));
        current.save(runtime)?;
    }
    for path in &removable {
        ownership::guard(&current, path)?;
        remove_path(runtime, path)?;
    }
    if removable.iter().any(|path| path.symlink_metadata().is_ok()) {
        bail!("removal returned before every recorded path disappeared");
    }
    current.status = "absent".to_owned();
    current.installed_at = now();
    current.installed_paths.clear();
    current.extra.remove("placement_fingerprints");
    current.extra.remove("prepared");
    current.release = None;
    current.source_revision = None;
    current.save(runtime)?;
    Ok(current)
}
