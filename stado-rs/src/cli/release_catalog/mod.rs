use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::Utc;
use clap::{Args, Subcommand};

use crate::release_pipeline::{
    self, CatalogSourceIdentity, ProductManifest, ReleaseCatalogEntry, SCHEMA_VERSION,
};

use super::CmdError;

mod adopt;
mod central;
mod checkout;
mod enroll;
mod publisher;

use central::sync_catalog;
use checkout::sync;
pub(crate) use enroll::enroll;

const CATALOG_PREFIX: &str = "release-catalog";

#[derive(Args)]
pub struct CatalogArgs {
    #[command(subcommand)]
    command: CatalogCommands,
}

#[derive(Subcommand)]
enum CatalogCommands {
    /// Set up everything a checkout's release manifest needs from the fleet:
    /// its release publisher, the build secrets its platforms and deliveries
    /// read (declared for and granted to the workload agent), the running
    /// service's own consumer with exactly `runtime.grants` and its bearer on
    /// every rollout target, and a check that every required platform
    /// declares post-build tests. `build submit` and `release submit` run the
    /// same steps before their first write.
    Enroll {
        /// The product checkout whose `.wisent-release.json` is read.
        checkout: PathBuf,
        #[arg(long)]
        json: bool,
    },
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
    /// Declare one product's release publisher across the fleet: mint its
    /// item on the vault owner, grant the release client access, declare it on
    /// every participating host, then reconcile each host's verifier grant.
    /// `build submit` and `release submit` run this themselves for a product
    /// this host has not declared, with this host as the client and the vault
    /// owner read from `skarbiec sync-status`; run it by hand only to declare
    /// further API targets or reloads.
    DeclarePublisher {
        /// The product, as its release manifest names it.
        product: String,
        /// The registry host whose vault is authoritative.
        #[arg(long)]
        owner: String,
        /// The registry host that runs `release submit`.
        #[arg(long)]
        client: String,
        /// Further hosts that serve the release API; repeat for several.
        #[arg(long = "target")]
        targets: Vec<String>,
        /// HOST=SERVICE: a managed unit whose process caches the publisher
        /// table for its lifetime, reconciled after the declaration lands
        /// on that host; repeat for several.
        #[arg(long = "reload")]
        reloads: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Add an application checkout to the release pipeline: write its
    /// release manifest and scripts from what its project states, declare its
    /// publisher, register it. Without --apply only the plan is printed.
    Adopt(adopt::AdoptArgs),
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

/// `catalog enroll`: the enrollment `build submit` runs, for one checkout's
/// manifest as it is in the working tree, reported step by step.
async fn enroll_checkout(checkout: &std::path::Path, json: bool) -> Result<(), CmdError> {
    let path = checkout.join(release_pipeline::PRODUCT_MANIFEST);
    let bytes = std::fs::read(&path)
        .map_err(|error| CmdError::click(format!("cannot read {}: {error}", path.display())))?;
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::click)?
    else {
        return Err(CmdError::click(format!(
            "{} declares releases:false; there is nothing to enroll",
            path.display()
        )));
    };
    let enrollment = enroll(&manifest).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "product": manifest.product,
                "steps": enrollment.steps,
            }))?
        );
    } else {
        for step in &enrollment.steps {
            println!("{}: {step}", manifest.product);
        }
    }
    untested_refusal(&manifest.product, &enrollment.untested)
}

/// The refusal for required platforms without post-build tests: their builds
/// pass, and no task they carry can ever be qualified.
pub(crate) fn untested_refusal(product: &str, untested: &[String]) -> Result<(), CmdError> {
    if untested.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{product}: required platform(s) {} declare no post-build tests, so no build of them can \
         qualify a task (it stays awaiting_tests); add a `tests` list of real product journeys to \
         each in .wisent-release.json",
        untested.join(", ")
    )))
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
        CatalogCommands::Enroll { checkout, json } => enroll_checkout(&checkout, json).await,
        CatalogCommands::DeclarePublisher {
            product,
            owner,
            client,
            targets,
            reloads,
            json,
        } => {
            publisher::declare_publisher(&product, &owner, &client, &targets, &reloads, json).await
        }
        CatalogCommands::Adopt(args) => adopt::run(args).await,
    }
}
