use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::Utc;
use clap::{Args, Subcommand};

use crate::release_control;
use crate::release_pipeline::{
    self, CatalogSourceIdentity, ProductManifest, ReleaseCatalogEntry, PRODUCT_MANIFEST,
    SCHEMA_VERSION,
};

use super::CmdError;

const CATALOG_PREFIX: &str = "release-catalog";

#[derive(Args)]
pub struct CatalogArgs {
    #[command(subcommand)]
    command: CatalogCommands,
}

#[derive(Subcommand)]
enum CatalogCommands {
    /// Register products from checked-out manifests or one central catalog.
    Sync {
        #[arg(long, required_unless_present = "catalog", conflicts_with = "catalog")]
        root: Option<PathBuf>,
        #[arg(long, required_unless_present = "root", conflicts_with = "root")]
        catalog: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Audit Stado's catalog without contacting repository hosts.
    Audit {
        #[arg(long)]
        json: bool,
    },
}

fn product(manifest: &ProductManifest) -> &str {
    match manifest {
        ProductManifest::Release(value) => &value.product,
        ProductManifest::NonRelease(value) => &value.product,
    }
}

fn catalog_uri(product: &str) -> String {
    format!("stado://system/{CATALOG_PREFIX}/{product}.json")
}

pub(crate) async fn publish_entry(
    manifest: ProductManifest,
    manifest_sha256: String,
    source: Option<CatalogSourceIdentity>,
) -> Result<ReleaseCatalogEntry, CmdError> {
    let product = product(&manifest).to_string();
    let mut entry = ReleaseCatalogEntry {
        schema_version: SCHEMA_VERSION,
        product: product.clone(),
        manifest_sha256,
        manifest,
        source,
        recorded_at: Utc::now().to_rfc3339(),
    };
    release_pipeline::validate_catalog_entry(&entry).map_err(CmdError::click)?;
    let uri = catalog_uri(&product);
    if let Some((existing, version)) = super::storage::fetch_object_versioned(&uri).await? {
        if let Ok(old) = serde_json::from_slice::<ReleaseCatalogEntry>(&existing) {
            if release_pipeline::validate_catalog_entry(&old).is_ok()
                && old.manifest == entry.manifest
            {
                if entry.source.is_none() || old.source == entry.source {
                    return Ok(old);
                }
                // A catalog import owns product policy; a release submission
                // adds source identity without rewriting that policy record.
                entry.manifest_sha256 = old.manifest_sha256;
            }
        }
        let bytes = serde_json::to_vec(&entry)?;
        super::storage::compare_and_swap_object(&uri, &bytes, "application/json", &version).await?;
    } else {
        let bytes = serde_json::to_vec(&entry)?;
        let temporary = tempfile::NamedTempFile::new()?;
        std::fs::write(temporary.path(), &bytes)?;
        super::storage::store_object(
            &uri,
            &temporary.path().display().to_string(),
            "application/json",
            true,
        )
        .await?;
    }
    Ok(entry)
}

fn scan(root: &Path, found: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    let mut entries: Vec<_> = std::fs::read_dir(root)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type()?.is_dir() {
            if matches!(
                name.as_ref(),
                ".git" | "target" | "node_modules" | ".venv" | "dist" | "build"
            ) {
                continue;
            }
            scan(&path, found)?;
        } else if name == PRODUCT_MANIFEST {
            found.push(path);
        }
    }
    Ok(())
}

async fn sync(root: &Path, json: bool) -> Result<(), CmdError> {
    let root = root.canonicalize()?;
    let mut paths = Vec::new();
    scan(&root, &mut paths)?;
    let mut declarations = BTreeMap::new();
    for path in paths {
        let bytes = std::fs::read(&path)?;
        let manifest = release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::click)?;
        let name = product(&manifest).to_string();
        if declarations
            .insert(name.clone(), (manifest, bytes))
            .is_some()
        {
            return Err(CmdError::click(format!(
                "catalog sync found duplicate product {name:?}"
            )));
        }
    }
    if declarations.is_empty() {
        return Err(CmdError::click(format!(
            "{} contains no {PRODUCT_MANIFEST}",
            root.display()
        )));
    }
    let mut entries = Vec::new();
    for (_, (manifest, bytes)) in declarations {
        entries.push(publish_entry(manifest, release_control::sha256_bytes(&bytes), None).await?);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for entry in entries {
            println!(
                "cataloged {} manifest={}",
                entry.product, entry.manifest_sha256
            );
        }
    }
    Ok(())
}

fn print_entries(entries: &[ReleaseCatalogEntry], json: bool) -> Result<(), CmdError> {
    if json {
        println!("{}", serde_json::to_string_pretty(entries)?);
    } else {
        for entry in entries {
            println!(
                "cataloged {} manifest={}",
                entry.product, entry.manifest_sha256
            );
        }
    }
    Ok(())
}

async fn sync_catalog(path: &Path, json: bool) -> Result<(), CmdError> {
    let bytes = std::fs::read(path)?;
    let document: serde_json::Value = serde_json::from_slice(&bytes)?;
    let repositories = document
        .get("repositories")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| CmdError::click("central catalog repositories must be an array"))?;
    let mut products = BTreeSet::new();
    let mut entries = Vec::new();
    for repository in repositories {
        let repository_name = repository
            .get("repository")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unknown repository>");
        let manifest_value = repository
            .get("manifest")
            .ok_or_else(|| CmdError::click("central catalog entry is missing manifest"))?;
        let manifest_bytes = serde_json::to_vec(manifest_value)?;
        let manifest = release_pipeline::parse_product_manifest(&manifest_bytes)
            .map_err(|error| CmdError::click(format!("{repository_name}: {error}")))?;
        let name = product(&manifest).to_string();
        if !products.insert(name.clone()) {
            return Err(CmdError::click(format!(
                "central catalog contains duplicate product {name:?}"
            )));
        }
        let declared_product = repository
            .get("product")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CmdError::click("central catalog entry is missing product"))?;
        if declared_product != name {
            return Err(CmdError::click(format!(
                "central catalog product {declared_product:?} disagrees with manifest {name:?}"
            )));
        }
        let manifest_sha256 = repository
            .get("manifest_sha256")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CmdError::click("central catalog entry is missing manifest_sha256"))?;
        let entry = publish_entry(manifest, manifest_sha256.to_string(), None)
            .await
            .map_err(|error| CmdError::click(format!("{repository_name}: {error}")))?;
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(CmdError::click(
            "central catalog contains no repository entries",
        ));
    }
    print_entries(&entries, json)
}

async fn audit(json: bool) -> Result<(), CmdError> {
    let uris = super::storage::list_object_uris("system", &format!("{CATALOG_PREFIX}/")).await?;
    let mut products = BTreeSet::new();
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for uri in uris {
        match super::storage::fetch_object(&uri).await.and_then(|bytes| {
            let entry: ReleaseCatalogEntry = serde_json::from_slice(&bytes)?;
            release_pipeline::validate_catalog_entry(&entry).map_err(CmdError::click)?;
            if uri != catalog_uri(&entry.product) {
                return Err(CmdError::click(
                    "catalog entry product disagrees with object coordinate",
                ));
            }
            Ok(entry)
        }) {
            Ok(entry) if products.insert(entry.product.clone()) => entries.push(entry),
            Ok(entry) => failures.push(format!("duplicate catalog product {}", entry.product)),
            Err(error) => failures.push(format!("{uri}: {error}")),
        }
    }
    if entries.is_empty() {
        failures.push("release catalog is silent: it contains no explicit product entries".into());
    }
    let report = serde_json::json!({
        "catalog": "stado://system/release-catalog/",
        "products": entries,
        "failures": failures,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "catalog products={} failures={}",
            entries.len(),
            failures.len()
        );
        for failure in &failures {
            eprintln!("catalog refusal: {failure}");
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CmdError::click(
            "release catalog audit refused malformed, duplicate, or silent entries",
        ))
    }
}

pub async fn dispatch(args: CatalogArgs) -> Result<(), CmdError> {
    match args.command {
        CatalogCommands::Sync {
            root,
            catalog,
            json,
        } => match (root, catalog) {
            (Some(root), None) => sync(&root, json).await,
            (None, Some(catalog)) => sync_catalog(&catalog, json).await,
            _ => Err(CmdError::click(
                "catalog sync requires exactly one of --root or --catalog",
            )),
        },
        CatalogCommands::Audit { json } => audit(json).await,
    }
}
