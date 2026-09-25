pub mod lifecycle;
pub(super) mod ownership;

use super::plan::{Placement, Prepared};
use crate::{
    common::{copy_tree, now, relative, Runtime},
    paths, signing,
    state::{Backup, ProductState},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub fn destination(runtime: &Runtime, path: &Path) -> Result<()> {
    let relative_path = path
        .strip_prefix(&runtime.home)
        .context("installation destination is outside HOME")?;
    relative(relative_path)?;
    if !path.starts_with(runtime.home.join(".stado"))
        && !path.starts_with(runtime.home.join(".local/bin"))
        && !path.starts_with(runtime.home.join("Applications"))
    {
        bail!("unowned installation destination {}", path.display());
    }
    let home = runtime.home.canonicalize()?;
    let mut parent = path.parent().context("destination has no parent")?;
    while !parent.exists() {
        parent = parent
            .parent()
            .context("destination has no existing ancestor")?;
    }
    if !parent.canonicalize()?.starts_with(&home) {
        bail!("installation parent escapes HOME: {}", path.display());
    }
    Ok(())
}

// The lifecycle entrypoint holds the product writer through preparation, placement and activation.
pub fn commit(
    runtime: &Runtime,
    product: &str,
    surface: &str,
    host: Option<&str>,
    recipe: &Value,
    plan: Prepared,
) -> Result<ProductState> {
    let _ownership = ownership::writer(runtime)?;
    let existing = ProductState::load(runtime, product, surface)?;
    if existing
        .as_ref()
        .is_some_and(|state| state.status == "removing" || state.status == "rolling_back")
    {
        bail!("finish the recorded removal or rollback before installing another artifact");
    }
    let shared = ownership::shared(runtime, product, surface)?;
    let owned = paths::owners(runtime)?;
    let mut selected = BTreeSet::new();
    let mut fingerprints = serde_json::Map::new();
    for placement in &plan.placements {
        destination(runtime, &placement.destination)?;
        paths::admit_name(&placement.destination, &placement.source, runtime)?;
        if !selected.insert(placement.destination.clone()) {
            bail!("two artifacts target {}", placement.destination.display());
        }
        let resolved = placement
            .destination
            .canonicalize()
            .unwrap_or(placement.destination.clone());
        if owned
            .get(&resolved)
            .is_some_and(|previous| previous.split_once('/').map(|(id, _)| id) != Some(product))
        {
            bail!(
                "{} belongs to another recorded product",
                placement.destination.display()
            );
        }
        if !placement.symbolic && plan.release.is_none() {
            signing::prepare(&placement.source, &placement.destination, product)?;
        }
        let fingerprint = placement.fingerprint()?;
        if ownership::overlaps(&placement.destination, &shared) {
            let actual = Placement {
                source: placement.destination.clone(),
                destination: placement.destination.clone(),
                symbolic: false,
            }
            .fingerprint()?;
            if actual != fingerprint {
                bail!(
                    "installation would change a path another surface still owns: {}",
                    placement.destination.display()
                );
            }
        }
        fingerprints.insert(
            placement.destination.to_string_lossy().into_owned(),
            fingerprint,
        );
    }
    let retired: Vec<PathBuf> = if let Some(state) = existing
        .as_ref()
        .filter(|state| state.status == "installing")
    {
        serde_json::from_value(
            state
                .extra
                .get("retired_paths")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?
    } else {
        existing
            .as_ref()
            .map(|state| {
                state
                    .installed_paths
                    .iter()
                    .filter(|path| !selected.contains(*path) && !ownership::overlaps(path, &shared))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    };
    for path in &retired {
        if ownership::overlaps(path, &shared) {
            bail!(
                "a retired path became owned by another surface: {}",
                path.display()
            );
        }
        if selected.iter().any(|target| target.starts_with(path)) {
            bail!("retired path {} contains a new artifact", path.display());
        }
    }
    if plan.placements.is_empty() {
        bail!("installation recipe produced no owned files");
    }
    let fingerprints = Value::Object(fingerprints);
    let state = if let Some(incomplete) = existing
        .as_ref()
        .filter(|state| state.status == "installing")
    {
        if incomplete.source_revision.as_deref() != Some(plan.source_revision.as_str())
            || incomplete.recipe != *recipe
            || incomplete.extra.get("placement_fingerprints") != Some(&fingerprints)
            || incomplete.release.as_ref().map(|r| &r["artifact"])
                != plan.release.as_ref().map(|r| &r["artifact"])
        {
            bail!("unfinished installation has another source or artifact; roll it back before selecting a replacement");
        }
        incomplete.clone()
    } else {
        let mut replaced = retired.clone();
        for placement in &plan.placements {
            if placement.source == placement.destination
                && !placement.symbolic
                && !existing
                    .as_ref()
                    .is_some_and(|state| state.installed_paths.contains(&placement.destination))
            {
                continue;
            }
            replaced.push(placement.destination.clone());
        }
        let backups = backup(runtime, product, &replaced)?;
        let mut extra = BTreeMap::new();
        extra.insert("placement_fingerprints".to_owned(), fingerprints);
        extra.insert("prepared".to_owned(), serde_json::to_value(&plan)?);
        extra.insert("retired_paths".to_owned(), json!(retired));
        extra.insert(
            "backup_fingerprints".to_owned(),
            backup_fingerprints(&backups)?,
        );
        ProductState {
            product: product.to_owned(),
            surface: surface.to_owned(),
            status: "installing".to_owned(),
            installed_at: now(),
            recipe: recipe.clone(),
            installed_paths: plan
                .placements
                .iter()
                .map(|p| p.destination.clone())
                .collect(),
            backups,
            host: host.map(str::to_owned),
            source_revision: Some(plan.source_revision.clone()),
            previous: existing.map(|existing| Box::new(existing.without_previous())),
            source_directory: plan.source_directory.clone(),
            release: plan.release.clone(),
            extra,
        }
    };
    state.save(runtime)?;
    for placement in &plan.placements {
        destination(runtime, &placement.destination)?;
        ownership::guard(&state, &placement.destination)?;
        placement.place()?;
        sync_tree(&placement.destination)?;
    }
    for path in &retired {
        ownership::guard(&state, path)?;
        remove_path(runtime, path)?;
    }
    ownership::verify(&state)?;
    Ok(state)
}

pub fn backup(runtime: &Runtime, product: &str, paths: &[PathBuf]) -> Result<Vec<Backup>> {
    let root = runtime
        .home
        .join(".stado/products")
        .join(product)
        .join("backups")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    }
    let mut backups = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        destination(runtime, path)?;
        match path.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let directory = root.join(index.to_string());
        fs::create_dir(&directory)?;
        let backup = directory.join(path.file_name().context("backup source has no filename")?);
        copy_tree(path, &backup)?;
        sync_tree(&backup)?;
        fs::File::open(&directory)?.sync_all()?;
        backups.push(Backup {
            path: path.clone(),
            backup,
        });
    }
    fs::File::open(&root)?.sync_all()?;
    Ok(backups)
}

pub fn backup_fingerprints(backups: &[Backup]) -> Result<Value> {
    let mut fingerprints = serde_json::Map::new();
    for saved in backups {
        fingerprints.insert(
            saved.backup.to_string_lossy().into_owned(),
            Placement {
                source: saved.backup.clone(),
                destination: saved.path.clone(),
                symbolic: false,
            }
            .fingerprint()?,
        );
    }
    Ok(Value::Object(fingerprints))
}

fn sync_tree(path: &Path) -> Result<()> {
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            sync_tree(&entry?.path())?;
        }
    }
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

pub fn remove_path(runtime: &Runtime, path: &Path) -> Result<()> {
    destination(runtime, path)?;
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}
