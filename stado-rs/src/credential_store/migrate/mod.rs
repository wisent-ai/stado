//! Verified credential-store migration. The persisted selector is the source;
//! `STADO_CREDENTIALS_STORE` (or an explicit CLI destination) is the target.
//! Normal credential access refuses a selector mismatch, so no process can
//! silently start against an empty store after an environment change.
mod selector;

use super::write::{delete_item_at, list_items_at, read_item_at, write_item_at};
use super::{configured_selector, parse_selector, requested_selector, Backend, ENV_STORE};
use crate::skarbiec::SkarbiecError;
use selector::persist_selector;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct MigrationReport {
    pub source: String,
    pub destination: String,
    /// How many items were copied and cleared from the source; `None` when
    /// the source and destination are one store and nothing was migrated.
    pub moved_items: Option<usize>,
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

async fn snapshot(
    backend: &Backend,
    url: &str,
    consumer: &str,
    token_file: &str,
    grant_mode: crate::skarbiec::GrantMode,
) -> Result<Vec<SnapshotItem>, SkarbiecError> {
    let mut snapshot = Vec::new();
    for info in list_items_at(backend, url, consumer, token_file, grant_mode).await? {
        if info.deleted {
            continue;
        }
        // The copy is written under the type the source states; an item it
        // lists with none is refused by id, never rewritten under a guess.
        let Some(item_type) = info.item_type else {
            return Err(deployment(format!(
                "credential item {} is listed without a type; the migration cannot write it to the destination as any type",
                info.id
            )));
        };
        let value = read_item_at(backend, url, consumer, token_file, grant_mode, &info.id).await?;
        snapshot.push(SnapshotItem {
            id: info.id,
            item_type,
            value,
        });
    }
    snapshot.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(snapshot)
}

async fn clear_items(backend: &Backend, items: &[SnapshotItem]) -> Result<(), SkarbiecError> {
    let mut first_error = None;
    for item in items.iter().rev() {
        if let Err(error) = delete_item_at(backend, &item.id).await {
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

async fn restore_items(backend: &Backend, items: &[SnapshotItem]) -> Result<(), SkarbiecError> {
    for item in items {
        write_item_at(
            backend,
            &item.id,
            &item.item_type,
            &item.value,
            &serde_json::json!({}),
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
    grant_mode: crate::skarbiec::GrantMode,
) -> Result<MigrationReport, SkarbiecError> {
    let source_raw = configured_selector()?;
    let requested_raw = requested_selector()?;
    // `--to` names the destination when given; else it is the store the
    // environment requests.
    let destination_raw = match explicit_destination {
        Some(explicit) => explicit,
        None => requested_raw.as_str(),
    }
    .trim();
    if explicit_destination.is_none() && source_raw == requested_raw {
        return Err(deployment(format!(
            "no credential-store change is pending; set {ENV_STORE} or pass --to"
        )));
    }
    if let Some(environment) = crate::config_file::variable(ENV_STORE).map_err(deployment)? {
        let environment = environment.trim();
        if !environment.is_empty()
            && explicit_destination.is_some_and(|value| value.trim() != environment)
        {
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
            moved_items: None,
        });
    }

    let items = snapshot(&source, url, consumer, token_file, grant_mode).await?;
    let destination_items =
        list_items_at(&destination, url, consumer, token_file, grant_mode).await?;
    if destination_items.iter().any(|item| !item.deleted) {
        return Err(deployment(format!(
            "destination {} is not empty; refusing to overwrite credentials",
            destination.locator()
        )));
    }

    if let Err(copy_error) = restore_items(&destination, &items).await {
        return Err(withdrawn(&destination, &items, copy_error).await);
    }
    let copied = match snapshot(&destination, url, consumer, token_file, grant_mode).await {
        Ok(copied) => copied,
        Err(verification_error) => {
            return Err(withdrawn(&destination, &items, verification_error).await)
        }
    };
    let verified = copied.len() == items.len()
        && copied.iter().zip(&items).all(|(left, right)| {
            left.id == right.id && left.item_type == right.item_type && left.value == right.value
        });
    if !verified {
        let differs = deployment("destination verification differs from the source snapshot");
        return Err(withdrawn(&destination, &items, differs).await);
    }
    if let Err(config_error) = persist_selector(&destination.locator()) {
        return Err(withdrawn(&destination, &items, config_error).await);
    }

    if let Err(delete_error) = clear_items(&source, &items).await {
        let source_restore = restore_items(&source, &items).await;
        let selector_restore = persist_selector(&source.locator());
        let destination_cleanup = clear_items(&destination, &items).await;
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
        moved_items: Some(items.len()),
    })
}

/// A migration stopped after copying into `to`: the copies are withdrawn, and any that remain (the
/// next migration refuses a non-empty destination) are named with their cause.
async fn withdrawn(to: &Backend, items: &[SnapshotItem], error: SkarbiecError) -> SkarbiecError {
    match clear_items(to, items).await {
        Ok(()) => error,
        Err(cleanup) => deployment(format!(
            "{error}; the copies left in {} could not be removed: {cleanup}",
            to.locator()
        )),
    }
}
