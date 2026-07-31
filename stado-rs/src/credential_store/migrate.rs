//! Verified credential-store migration. The persisted selector is the source;
//! `STADO_CREDENTIALS_STORE` (or an explicit CLI destination) is the target.
//! Normal credential access refuses a selector mismatch, so no process can
//! silently start against an empty store after an environment change.
#[cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};
use serde::Serialize;
use serde_json::{Map, Value};
use super::write::{delete_item_at, list_items_at, read_item_at, write_item_at};
use super::{configured_selector, parse_selector, requested_selector, Backend, ENV_STORE};
use crate::skarbiec::SkarbiecError;

#[derive(Debug, Serialize)]
pub struct MigrationReport {
    pub source: String,
    pub destination: String,
    pub moved_items: usize,
}

#[derive(Clone)]
struct SnapshotItem {
    id: String,
    item_type: String,
    value: Value,
}

fn deployment(detail: impl Into<String>) -> SkarbiecError {
    SkarbiecError::Deployment(detail.into())
}

fn config_path_for_write() -> Result<PathBuf, SkarbiecError> {
    if let Some(path) = crate::config_file::find_config_file() {
        return Ok(path);
    }
    if let Ok(raw) = std::env::var(crate::config_file::FILE_ENV) {
        if !raw.trim().is_empty() {
            return Ok(crate::config_file::expand_tilde(raw.trim()));
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| deployment("HOME is not set; cannot persist credentials.store"))?;
    Ok(home.join(".stado").join("config.json"))
}

#[cfg(unix)]
fn write_config(path: &Path, body: &[u8]) -> Result<(), SkarbiecError> {
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| deployment(format!("config path {} has no parent", path.display())))?;
    std::fs::create_dir_all(parent).map_err(|source| {
        deployment(format!(
            "cannot create config directory {}: {source}",
            parent.display()
        ))
    })?;
    let temporary = parent.join(format!(".stado-config-{}.tmp", std::process::id()));
    let owner_mode = u32::from_str_radix("600", u32::from(u8::BITS))
        .map_err(|source| deployment(source.to_string()))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    if let Ok(metadata) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        options.mode(metadata.permissions().mode());
    } else {
        options.mode(owner_mode);
    }
    let mut file = options.open(&temporary).map_err(|source| {
        deployment(format!(
            "cannot create temporary config {}: {source}",
            temporary.display()
        ))
    })?;
    file.write_all(body)
        .and_then(|_| file.sync_all())
        .map_err(|source| deployment(format!("cannot write config: {source}")))?;
    std::fs::rename(&temporary, path).map_err(|source| {
        let _ = std::fs::remove_file(&temporary);
        deployment(format!("cannot replace config {}: {source}", path.display()))
    })
}

#[cfg(not(unix))]
fn write_config(path: &Path, body: &[u8]) -> Result<(), SkarbiecError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| deployment(format!("cannot create config directory: {source}")))?;
    }
    std::fs::write(path, body)
        .map_err(|source| deployment(format!("cannot write config {}: {source}", path.display())))
}

fn persist_selector(locator: &str) -> Result<(), SkarbiecError> {
    let path = config_path_for_write()?;
    let mut document = if path.exists() {
        let body = std::fs::read_to_string(&path)
            .map_err(|source| deployment(format!("cannot read config {}: {source}", path.display())))?;
        serde_json::from_str::<Value>(&body)
            .map_err(|source| deployment(format!("config {} is invalid JSON: {source}", path.display())))?
    } else {
        Value::Object(Map::new())
    };
    let root = document
        .as_object_mut()
        .ok_or_else(|| deployment(format!("config {} must be a JSON object", path.display())))?;
    if !root.contains_key("schema_version") {
        root.insert(
            "schema_version".to_string(),
            Value::from(crate::config_file::SCHEMA_VERSION),
        );
    }
    match root.get_mut("credentials") {
        None => {
            root.insert(
                "credentials".to_string(),
                serde_json::json!({"store": locator}),
            );
        }
        Some(Value::Object(credentials)) => {
            credentials.insert("store".to_string(), Value::String(locator.to_string()));
        }
        Some(_) => {
            return Err(deployment(format!(
                "config {} field credentials must be an object",
                path.display()
            )));
        }
    }
    let body = format!(
        "{}\n",
        serde_json::to_string_pretty(&document)
            .map_err(|source| deployment(source.to_string()))?
    );
    write_config(&path, body.as_bytes())
}

async fn snapshot(
    backend: &Backend,
    url: &str,
    consumer: &str,
    token_file: &str,
) -> Result<Vec<SnapshotItem>, SkarbiecError> {
    let mut snapshot = Vec::new();
    for info in list_items_at(backend, url, consumer, token_file).await? {
        if info.deleted == Some(true) {
            continue;
        }
        let value = read_item_at(backend, url, consumer, token_file, &info.id).await?;
        snapshot.push(SnapshotItem {
            id: info.id,
            item_type: info.item_type.unwrap_or_else(|| "stado-secret".to_string()),
            value,
        });
    }
    snapshot.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(snapshot)
}

async fn clear_items(
    backend: &Backend,
    items: &[SnapshotItem],
    url: &str,
    consumer: &str,
    token_file: &str,
) -> Result<(), SkarbiecError> {
    let mut first_error = None;
    for item in items.iter().rev() {
        if let Err(error) =
            delete_item_at(backend, url, consumer, token_file, &item.id).await
        {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn restore_items(
    backend: &Backend,
    items: &[SnapshotItem],
    url: &str,
    consumer: &str,
    token_file: &str,
) -> Result<(), SkarbiecError> {
    for item in items {
        write_item_at(
            backend,
            url,
            consumer,
            token_file,
            &item.id,
            &item.item_type,
            &item.value,
        )
        .await?;
    }
    Ok(())
}

/// Move every active credential from the persisted backend to the requested
/// backend, verify exact values, commit the selector, then remove the source.
/// Any failure before completion restores the persisted selector and source.
pub async fn migrate(
    explicit_destination: Option<&str>,
    url: &str,
    consumer: &str,
    token_file: &str,
) -> Result<MigrationReport, SkarbiecError> {
    let source_raw = configured_selector()?;
    let requested_raw = requested_selector()?;
    let destination_raw = explicit_destination.unwrap_or(&requested_raw).trim();
    if explicit_destination.is_none() && source_raw == requested_raw {
        return Err(deployment(format!(
            "no credential-store change is pending; set {ENV_STORE} or pass --to"
        )));
    }
    if let Ok(environment) = std::env::var(ENV_STORE) {
        let environment = environment.trim();
        if !environment.is_empty() && explicit_destination.is_some_and(|value| value.trim() != environment) {
            return Err(deployment(format!(
                "--to {destination_raw:?} conflicts with {ENV_STORE}={environment:?}"
            )));
        }
    }

    let source = parse_selector(&source_raw)?;
    let destination = parse_selector(destination_raw)?;
    if source == destination {
        persist_selector(&destination.locator())?;
        return Ok(MigrationReport {
            source: source.locator(),
            destination: destination.locator(),
            moved_items: usize::default(),
        });
    }

    let items = snapshot(&source, url, consumer, token_file).await?;
    let destination_items = list_items_at(&destination, url, consumer, token_file).await?;
    if destination_items.iter().any(|item| item.deleted != Some(true)) {
        return Err(deployment(format!(
            "destination {} is not empty; refusing to overwrite credentials",
            destination.locator()
        )));
    }

    if let Err(copy_error) = restore_items(&destination, &items, url, consumer, token_file).await {
        let _ = clear_items(&destination, &items, url, consumer, token_file).await;
        return Err(copy_error);
    }
    let copied = match snapshot(&destination, url, consumer, token_file).await {
        Ok(copied) => copied,
        Err(verification_error) => {
            let _ = clear_items(&destination, &items, url, consumer, token_file).await;
            return Err(verification_error);
        }
    };
    let verified = copied.len() == items.len()
        && copied.iter().zip(&items).all(|(left, right)| {
            left.id == right.id
                && left.item_type == right.item_type
                && left.value == right.value
        });
    if !verified {
        let _ = clear_items(&destination, &items, url, consumer, token_file).await;
        return Err(deployment("destination verification differs from the source snapshot"));
    }
    if let Err(config_error) = persist_selector(&destination.locator()) {
        let _ = clear_items(&destination, &items, url, consumer, token_file).await;
        return Err(config_error);
    }

    if let Err(delete_error) = clear_items(&source, &items, url, consumer, token_file).await {
        let source_restore = restore_items(&source, &items, url, consumer, token_file).await;
        let selector_restore = persist_selector(&source.locator());
        let destination_cleanup = clear_items(&destination, &items, url, consumer, token_file).await;
        if source_restore.is_err() || selector_restore.is_err() || destination_cleanup.is_err() {
            return Err(deployment(format!(
                "source cleanup failed ({delete_error}); rollback was incomplete and requires operator recovery"
            )));
        }
        return Err(deployment(format!(
            "source cleanup failed ({delete_error}); migration was rolled back"
        )));
    }

    Ok(MigrationReport {
        source: source.locator(),
        destination: destination.locator(),
        moved_items: items.len(),
    })
}
