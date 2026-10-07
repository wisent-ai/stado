//! The canonical store owns catalogs and immutable, self-contained plan receipts.
//!
//! Every failure states its class where it is raised: a store failure keeps
//! the class the store's own conversion gives it, a stored document that no
//! longer decodes or validates is damaged state (`infra_down`), and a write
//! the catalog's version or a concurrent writer forbids is `refused`.
use super::constants::{CATALOG_PATH, PLAN_PREFIX, SCHEMA_VERSION};
use super::model::{Catalog, CatalogRecord, ExpansionReport};
use super::plan::validate;
use crate::cli::CmdError;
use crate::queue::JobStorage;

pub async fn read_catalog(store: &JobStorage) -> Result<CatalogRecord, CmdError> {
    let record = store
        .read_text_versioned(CATALOG_PATH)
        .await
        .map_err(|e| CmdError::from(e).within(format!("read {CATALOG_PATH}")))?;
    match record {
        None => Ok(CatalogRecord {
            version: None,
            catalog: Catalog::default(),
        }),
        Some(record) => {
            let catalog: Catalog = serde_json::from_str(&record.content).map_err(|e| {
                CmdError::unreachable(format!("stored {CATALOG_PATH} does not decode: {e}"))
            })?;
            validate::catalog(&catalog).map_err(|e| {
                CmdError::unreachable(format!("stored {CATALOG_PATH} is invalid: {e}"))
            })?;
            Ok(CatalogRecord {
                version: Some(record.version),
                catalog,
            })
        }
    }
}

pub async fn replace_catalog(
    store: &JobStorage,
    catalog: Catalog,
    expected: Option<&str>,
) -> Result<CatalogRecord, CmdError> {
    validate::catalog(&catalog).map_err(CmdError::refused)?;
    let current = read_catalog(store).await?;
    if current.version.as_deref() != expected {
        return Err(CmdError::refused(format!(
            "expansion catalog version conflict: expected {:?}, current {:?}; no write performed",
            expected, current.version
        )));
    }
    let content = serde_json::to_string(&catalog)?;
    if let Some(version) = expected {
        store
            .compare_and_swap_text(CATALOG_PATH, version, &content)
            .await
            .map_err(|e| CmdError::from(e).within(format!("replace {CATALOG_PATH}")))?;
    } else if !store
        .create_text_if_absent(CATALOG_PATH, &content)
        .await
        .map_err(|e| CmdError::from(e).within(format!("create {CATALOG_PATH}")))?
    {
        return Err(CmdError::refused(
            "expansion catalog was created concurrently; no overwrite performed",
        ));
    }
    let saved = read_catalog(store).await?;
    // A concurrent writer must not turn a successful write into a receipt for different inputs.
    if serde_json::to_string(&saved.catalog)? != content {
        return Err(CmdError::refused(
            "expansion catalog changed after write; read catalog before another replacement",
        ));
    }
    Ok(saved)
}

fn plan_path(id: &str) -> Result<String, CmdError> {
    let parsed =
        uuid::Uuid::parse_str(id).map_err(|_| CmdError::usage("expansion plan id must be a UUID"))?;
    if parsed.to_string() != id {
        return Err(CmdError::usage(
            "expansion plan id must use canonical UUID spelling",
        ));
    }
    Ok(format!("{PLAN_PREFIX}{id}.json"))
}

pub(crate) async fn save_plan(store: &JobStorage, report: &ExpansionReport) -> Result<(), CmdError> {
    let path = plan_path(&report.plan_id)?;
    let raw = serde_json::to_string(report)?;
    if !store
        .create_text_if_absent(&path, &raw)
        .await
        .map_err(|e| CmdError::from(e).within(format!("persist {path}")))?
    {
        return Err(CmdError::refused(format!(
            "expansion plan already exists: {}",
            report.plan_id
        )));
    }
    let persisted = store
        .download_text(&path)
        .await
        .map_err(|e| CmdError::from(e).within(format!("verify {path}")))?;
    if persisted.as_deref() != Some(raw.as_str()) {
        return Err(CmdError::unreachable(format!(
            "expansion plan read-back differs: {path}"
        )));
    }
    Ok(())
}

pub async fn read_plan(store: &JobStorage, id: &str) -> Result<ExpansionReport, CmdError> {
    let path = plan_path(id)?;
    let raw = store
        .download_text(&path)
        .await
        .map_err(|e| CmdError::from(e).within(format!("read {path}")))?
        .ok_or_else(|| CmdError::missing(format!("expansion plan not found: {id}")))?;
    let report: ExpansionReport = serde_json::from_str(&raw)
        .map_err(|e| CmdError::unreachable(format!("stored {path} does not decode: {e}")))?;
    if report.schema_version != SCHEMA_VERSION || report.plan_id != id {
        return Err(CmdError::unreachable(format!(
            "expansion plan identity or schema mismatch: {path}"
        )));
    }
    Ok(report)
}

pub async fn history(store: &JobStorage) -> Result<Vec<ExpansionReport>, CmdError> {
    let paths = store
        .list_paths(PLAN_PREFIX, 0)
        .await
        .map_err(|e| CmdError::from(e).within(format!("list {PLAN_PREFIX}")))?;
    let mut plans = Vec::new();
    for path in paths {
        let id = path
            .strip_prefix(PLAN_PREFIX)
            .and_then(|v| v.strip_suffix(".json"))
            .ok_or_else(|| CmdError::unreachable(format!("unexpected expansion plan path: {path}")))?;
        plans.push(read_plan(store, id).await?);
    }
    plans.sort_by(|a, b| {
        b.generated_at
            .cmp(&a.generated_at)
            .then_with(|| a.plan_id.cmp(&b.plan_id))
    });
    Ok(plans)
}
