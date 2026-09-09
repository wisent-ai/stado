use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::Utc;
use clap::{Args, Subcommand};

use crate::release_pipeline::{
    self, CatalogSourceIdentity, ProductManifest, ReleaseCatalogEntry, SCHEMA_VERSION,
};

use super::CmdError;

mod central;
mod checkout;

use central::sync_catalog;
use checkout::sync;

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

/// The human-readable form of one audit: the tally on stdout, then every
/// refusal on stderr, so a shell pipeline keeps the tally alone.
fn print_audit(entries: &[ReleaseCatalogEntry], failures: &[String]) {
    println!(
        "catalog products={} failures={}",
        entries.len(),
        failures.len()
    );
    for failure in failures {
        eprintln!("catalog refusal: {failure}");
    }
}

async fn audit(json: bool) -> Result<(), CmdError> {
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        CmdError::click(format!(
            "release catalog audit refused invalid release_api.publishers: {}",
            problems.join("; ")
        ))
    })?;
    let mut products = BTreeSet::new();
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for product in publishers.keys() {
        let uri = catalog_uri(product);
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
        print_audit(&entries, &failures);
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
