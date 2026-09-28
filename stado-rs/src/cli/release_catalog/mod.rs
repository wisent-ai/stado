use std::path::PathBuf;

use chrono::Utc;
use clap::{Args, Subcommand};

use crate::release_pipeline::{
    self, CatalogSourceIdentity, ProductManifest, ReleaseCatalogEntry, SCHEMA_VERSION,
};

use super::CmdError;

mod adopt;
mod audit;
mod central;
mod checkout;
mod enroll;
mod publisher;

pub(crate) use publisher::mint_bearer as fresh_bearer;

use central::sync_catalog;
use checkout::sync;
pub(crate) use enroll::{enroll, missing_programs_refusal, missing_step_programs};
pub(crate) use publisher::{fleet_hosts, this_host};

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
    /// read (declared for and granted to the workload agent), for a service
    /// its rollout policy (created from runtime.port when absent) and its own
    /// consumer with exactly `runtime.grants` and its bearer on every rollout
    /// target, a check that every required platform declares post-build
    /// tests, and the product's entry in the release catalog the daily batch
    /// builds from. `build submit` and `release submit` run the same steps
    /// before their first write.
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
    /// Audit Stado's catalog without contacting repository hosts: every declared
    /// publisher must have a catalog entry, and every catalogued release must
    /// declare post-build tests on each required platform, or its builds never
    /// qualify a task. Exits nonzero and names each finding on any refusal.
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
        /// HOST=SERVICE: the host Stado process on HOST, whose release API
        /// caches the publisher table for its lifetime, reconciled after the
        /// declaration lands on that host; repeat for several hosts. Any
        /// other unit, a retired one included, is refused.
        #[arg(long = "reload")]
        reloads: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a retired product's release publisher declaration from the
    /// vault owner, this host and every --target, then reconcile each host's
    /// release verifier. Refuses while the release catalog still holds the
    /// product. The vault item stays, so a returning product re-declares.
    WithdrawPublisher {
        /// The product, as its release manifest named it.
        product: String,
        /// Further hosts that serve the release API; repeat for several.
        #[arg(long = "target")]
        targets: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Add an application checkout to the release pipeline: write its
    /// release manifest and scripts from what its project states, declare its
    /// publisher, register it. Without --apply only the plan is printed.
    Adopt(adopt::AdoptArgs),
    /// Make one committed revision of another repository an immutable build
    /// input of the product in CHECKOUT: archive it, store it create-only
    /// under the product's sources, and write `inputs.<name>` of its
    /// `.wisent-release.json`. The build reads it from WISENT_INPUT_<NAME>_DIR.
    PinInput {
        /// The product checkout whose `.wisent-release.json` gains the input.
        checkout: PathBuf,
        /// The input's name, mount and environment key (lowercase, digits, hyphens).
        #[arg(long)]
        name: String,
        /// The repository checkout the revision is archived from.
        #[arg(long)]
        source: PathBuf,
        /// The commit to pin; a tag or branch resolves to its commit.
        #[arg(long)]
        revision: String,
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

/// `catalog enroll`: the enrollment `build submit` runs, for one checkout's
/// manifest as it is in the working tree, reported step by step, and then the
/// product's catalog entry, which is what the daily batch builds from: an
/// enrolled product the catalog does not hold is never built.
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
    enroll::missing_programs_refusal(
        &manifest.product,
        &enroll::missing_step_programs(&manifest, checkout),
    )?;
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
    untested_refusal(&manifest.product, &enrollment.untested)?;
    sync(checkout, json).await
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
        CatalogCommands::Audit { json } => audit::audit(json).await,
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
        CatalogCommands::WithdrawPublisher {
            product,
            targets,
            json,
        } => publisher::withdraw_publisher(&product, &targets, json).await,
        CatalogCommands::Adopt(args) => adopt::run(args).await,
        CatalogCommands::PinInput {
            checkout,
            name,
            source,
            revision,
            json,
        } => publisher::pin_input(&checkout, &name, &source, &revision, json).await,
    }
}
