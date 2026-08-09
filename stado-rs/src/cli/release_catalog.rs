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
    /// Register checked-out products under one local root in Stado's catalog.
    Sync {
        #[arg(long)]
        root: PathBuf,
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
    let entry = ReleaseCatalogEntry {
        schema_version: SCHEMA_VERSION,
        product: product.clone(),
        manifest_sha256,
        manifest,
        source,
        recorded_at: Utc::now().to_rfc3339(),
    };
    release_pipeline::validate_catalog_entry(&entry).map_err(CmdError::click)?;
    let bytes = serde_json::to_vec(&entry)?;
    let uri = catalog_uri(&product);
    if let Some((existing, version)) = super::storage::fetch_object_versioned(&uri).await? {
        let old: ReleaseCatalogEntry = serde_json::from_slice(&existing)?;
        release_pipeline::validate_catalog_entry(&old).map_err(CmdError::click)?;
        if old == entry {
            return Ok(entry);
        }
        super::storage::compare_and_swap_object(&uri, &bytes, "application/json", &version).await?;
    } else {
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
        CatalogCommands::Sync { root, json } => sync(&root, json).await,
        CatalogCommands::Audit { json } => audit(json).await,
    }
}
