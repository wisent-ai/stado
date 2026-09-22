//! The canonical store owns catalogs and immutable, self-contained plan receipts.
use super::constants::{CATALOG_PATH, PLAN_PREFIX, SCHEMA_VERSION};
use super::model::{Catalog, CatalogRecord, ExpansionReport};
use super::plan::validate;
use crate::queue::JobStorage;

pub async fn read_catalog(store: &JobStorage) -> Result<CatalogRecord, String> {
    let record = store
        .read_text_versioned(CATALOG_PATH)
        .await
        .map_err(|e| format!("read {CATALOG_PATH}: {e}"))?;
    match record {
        None => Ok(CatalogRecord {
            version: None,
            catalog: Catalog::default(),
        }),
        Some(record) => {
            let catalog: Catalog = serde_json::from_str(&record.content)
                .map_err(|e| format!("decode {CATALOG_PATH}: {e}"))?;
            validate::catalog(&catalog)?;
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
) -> Result<CatalogRecord, String> {
    validate::catalog(&catalog)?;
    let current = read_catalog(store).await?;
    if current.version.as_deref() != expected {
        return Err(format!(
            "expansion catalog version conflict: expected {:?}, current {:?}; no write performed",
            expected, current.version
        ));
    }
    let content = serde_json::to_string(&catalog).map_err(|e| e.to_string())?;
    if let Some(version) = expected {
        store
            .compare_and_swap_text(CATALOG_PATH, version, &content)
            .await
            .map_err(|e| format!("replace {CATALOG_PATH}: {e}"))?;
    } else if !store
        .create_text_if_absent(CATALOG_PATH, &content)
        .await
        .map_err(|e| format!("create {CATALOG_PATH}: {e}"))?
    {
        return Err("expansion catalog was created concurrently; no overwrite performed".into());
    }
    let saved = read_catalog(store).await?;
    // A concurrent writer must not turn a successful write into a receipt for different inputs.
    if serde_json::to_string(&saved.catalog).map_err(|e| e.to_string())? != content {
        return Err(
            "expansion catalog changed after write; read catalog before another replacement".into(),
        );
    }
    Ok(saved)
}

fn plan_path(id: &str) -> Result<String, String> {
    let parsed =
        uuid::Uuid::parse_str(id).map_err(|_| "expansion plan id must be a UUID".to_string())?;
    if parsed.to_string() != id {
        return Err("expansion plan id must use canonical UUID spelling".into());
    }
    Ok(format!("{PLAN_PREFIX}{id}.json"))
}

pub(crate) async fn save_plan(store: &JobStorage, report: &ExpansionReport) -> Result<(), String> {
    let path = plan_path(&report.plan_id)?;
    let raw = serde_json::to_string(report).map_err(|e| e.to_string())?;
    if !store
        .create_text_if_absent(&path, &raw)
        .await
        .map_err(|e| format!("persist {path}: {e}"))?
    {
        return Err(format!("expansion plan already exists: {}", report.plan_id));
    }
    let persisted = store
        .download_text(&path)
        .await
        .map_err(|e| format!("verify {path}: {e}"))?;
    if persisted.as_deref() != Some(raw.as_str()) {
        return Err(format!("expansion plan read-back differs: {path}"));
    }
    Ok(())
}

pub async fn read_plan(store: &JobStorage, id: &str) -> Result<ExpansionReport, String> {
    let path = plan_path(id)?;
    let raw = store
        .download_text(&path)
        .await
        .map_err(|e| format!("read {path}: {e}"))?
        .ok_or_else(|| format!("expansion plan not found: {id}"))?;
    let report: ExpansionReport =
        serde_json::from_str(&raw).map_err(|e| format!("decode {path}: {e}"))?;
    if report.schema_version != SCHEMA_VERSION || report.plan_id != id {
        return Err(format!(
            "expansion plan identity or schema mismatch: {path}"
        ));
    }
    Ok(report)
}

pub async fn history(store: &JobStorage) -> Result<Vec<ExpansionReport>, String> {
    let paths = store
        .list_paths(PLAN_PREFIX, usize::default())
        .await
        .map_err(|e| format!("list {PLAN_PREFIX}: {e}"))?;
    let mut plans = Vec::new();
    for path in paths {
        let id = path
            .strip_prefix(PLAN_PREFIX)
            .and_then(|v| v.strip_suffix(".json"))
            .ok_or_else(|| format!("unexpected expansion plan path: {path}"))?;
        plans.push(read_plan(store, id).await?);
    }
    plans.sort_by(|a, b| {
        b.generated_at
            .cmp(&a.generated_at)
            .then_with(|| a.plan_id.cmp(&b.plan_id))
    });
    Ok(plans)
}
