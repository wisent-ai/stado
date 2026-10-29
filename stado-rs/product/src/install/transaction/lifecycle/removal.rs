use super::super::{backup, backup_fingerprints, ownership, remove_path};
use crate::{
    catalog::{self, text},
    common::{now, Runtime},
    install::services,
    state::{self, ProductState},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

/// Whether a recorded installation of `id` placed the program running now:
/// its removal takes away the binary every later removal runs
/// `stado service remove` through.
fn places_running(runtime: &Runtime, id: &str, running: Option<&Path>) -> Result<bool> {
    let Some(running) = running else {
        return Ok(false);
    };
    for surface in state::SURFACES {
        let Some(receipt) = ProductState::load(runtime, id, surface)? else {
            continue;
        };
        if receipt
            .installed_paths
            .iter()
            .any(|path| path.canonicalize().is_ok_and(|path| path == running))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `stado product remove PRODUCT...` without `--surface`: each named product
/// through [`remove_every`], the one whose installation placed the running
/// program last, so it is still there for the others. A product named twice
/// is refused before anything is removed, and a removal that fails names the
/// products already removed.
pub fn remove_products(
    runtime: &Runtime,
    document: &Value,
    ids: &[String],
    host: Option<&str>,
) -> Result<Vec<Value>> {
    let running = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .ok();
    let mut products = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        if ids[..index].contains(id) {
            bail!("product {id} is named twice; name each product once");
        }
        let product = catalog::product(document, id)?;
        products.push((
            places_running(runtime, id, running.as_deref())?,
            id,
            product,
        ));
    }
    products.sort_by_key(|(running, _, _)| *running);
    let mut removed = Vec::new();
    for (_, id, product) in products {
        let surfaces = remove_every(runtime, product, host).with_context(|| {
            let done: Vec<&Value> = removed.iter().map(|row: &Value| &row["product"]).collect();
            format!("removing {id}, after {done:?} were removed")
        })?;
        removed.push(json!({ "product": id, "surfaces": surfaces }));
    }
    Ok(removed)
}

/// `stado product remove PRODUCT` without `--surface`: every surface of the
/// product this machine records as installed, each through [`remove`], in
/// the reverse of the order [`state::SURFACES`] names them: a service is
/// removed through `stado service remove`, so it goes while the CLI that
/// removes it is still installed, and the CLI goes last. A surface whose
/// receipt says `absent` is left as it is; a product with no installed
/// surface at all is refused, naming where the receipts were read.
fn remove_every(
    runtime: &Runtime,
    product: &Value,
    host: Option<&str>,
) -> Result<Vec<ProductState>> {
    let id = text(product, "id")?;
    let mut installed = Vec::new();
    for surface in state::SURFACES.into_iter().rev() {
        let recorded = ProductState::load(runtime, id, surface)?;
        if recorded.is_some_and(|receipt| receipt.status != crate::state::InstallStatus::Absent) {
            installed.push(surface);
        }
    }
    if installed.is_empty() {
        bail!(
            "{id} has no installed surface on this machine: no receipt under {} records one; \
             `stado product status {id} --surface SURFACE` reads one surface",
            runtime.home.join(".stado/products").join(id).display()
        );
    }
    installed
        .into_iter()
        .map(|surface| remove(runtime, product, surface, host))
        .collect()
}

pub fn remove(
    runtime: &Runtime,
    product: &Value,
    surface: &str,
    host: Option<&str>,
) -> Result<ProductState> {
    let id = text(product, "id")?;
    let receipt = state::path(runtime, id, surface)?;
    let _writer = runtime.surface_lock(&receipt.with_extension("lock"))?;
    let _ownership = ownership::writer(runtime)?;
    let mut current = ProductState::load(runtime, id, surface)?.with_context(|| {
        format!(
            "{id} {surface} has no recorded installation: {} does not exist; \
             `stado product remove {id}` without --surface removes every installed surface",
            receipt.display()
        )
    })?;
    if current.status == crate::state::InstallStatus::Absent {
        return Ok(current);
    }
    if matches!(
        current.status,
        crate::state::InstallStatus::Installing | crate::state::InstallStatus::RollingBack
    ) {
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
    if current.status != crate::state::InstallStatus::Removing {
        ownership::verify(&current)
            .context("recorded files changed; refusing to delete unverified replacement files")?;
        let previous = current.clone().without_previous();
        current.backups = backup(runtime, id, &removable)?;
        current.extra.insert(
            "backup_fingerprints".to_owned(),
            backup_fingerprints(&current.backups)?,
        );
        current.previous = Some(Box::new(previous));
        current.status = crate::state::InstallStatus::Removing;
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
    current.status = crate::state::InstallStatus::Absent;
    current.installed_at = now();
    current.installed_paths.clear();
    current.extra.remove("placement_fingerprints");
    current.extra.remove("prepared");
    current.extra.remove("signing");
    current.release = None;
    current.source_revision = None;
    current.save(runtime)?;
    Ok(current)
}
