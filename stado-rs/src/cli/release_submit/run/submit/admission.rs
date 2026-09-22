//! Immutable source admission before the release starts its jobs.
use super::continue_run;
use crate::cli::release_submit::publish::signing::require_rollback_compatibility;
use crate::cli::release_submit::run::source::{
    committed_file, identity, immutable, queue_immutable, resolve_commit, run_path, run_uri,
    snapshot,
};
use crate::cli::release_submit::run::state::load;
use crate::cli::release_submit::ReleaseSubmitArgs;
use crate::cli::{release_catalog, release_cmd, CmdError};
use crate::release_control;
use crate::release_pipeline::{
    self, CatalogSourceIdentity, ProductManifest, ReleaseRun, ReleaseRunState, PRODUCT_MANIFEST,
};
use chrono::Utc;
use std::collections::BTreeMap;

const OBJECT_API_CATALOG_SERVICE: &str = "stado";
const OBJECT_API_REASON: &str =
    "release submission requires the canonical object store before its first write";
pub async fn submit(args: &ReleaseSubmitArgs) -> Result<(), CmdError> {
    let root = args.source.canonicalize()?;
    let commit = resolve_commit(&root, args.commit.as_deref())?;
    let manifest_bytes = committed_file(&root, &commit, PRODUCT_MANIFEST)?;
    let pm = release_pipeline::parse_product_manifest(&manifest_bytes).map_err(CmdError::click)?;
    let ProductManifest::Release(m) = pm.clone() else {
        return Err(CmdError::click("product declares releases:false"));
    };
    let declared = release_pipeline::declared_version(&m.version_source, |path| {
        committed_file(&root, &commit, path).map_err(|error| error.to_string())
    })
    .map_err(CmdError::click)?;
    if declared != args.version {
        return Err(CmdError::click(
            "--version disagrees with declared version source",
        ));
    }
    let channel = args.channel.into();
    if !m.promotion.channels.contains(&channel) {
        return Err(CmdError::click(
            "requested channel is forbidden by promotion policy",
        ));
    }
    require_rollback_compatibility(&m, &args.version).await?;
    // An explicit endpoint may be a Stado-managed loopback forward to the
    // control host. Only an absent endpoint means this caller owns the local
    // object daemon and must ensure it before publishing. Address the service
    // by its canonical catalog name: that entry owns both the stable object-API
    // unit identity and its host-local storage environment.
    if crate::config::stado_api_url().is_empty()
        && crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
            != Some(crate::capabilities::StorageAdapter::Local)
    {
        crate::cli::service::ensure_local_dependency(
            OBJECT_API_CATALOG_SERVICE,
            OBJECT_API_REASON,
            true,
        )
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "cannot ensure required service {OBJECT_API_CATALOG_SERVICE}: {error}"
            ))
        })?;
    }
    let archive = snapshot(&root, &commit)?;
    // Reserve every platform coordinate before this submission can become the
    // newest durable run. Delivery workers fence themselves against that
    // newest run. When the claim lived only in `publish`, a second source tree
    // could persist a newer run for the same version, fail later against the
    // first tree's immutable claim, and still make every valid delivery from
    // the first run refuse itself as superseded. Claiming in the manifest's
    // stable platform order makes that incompatible submission fail before it
    // can become a delivery fence.
    for platform in m.platforms.keys() {
        release_cmd::claim_release_coordinate(&m.product, &args.version, platform, &commit).await?;
    }
    let source_sha = release_control::sha256_bytes(&archive);
    let manifest_sha = release_control::sha256_bytes(&manifest_bytes);
    let source_uri = format!("stado://sources/{}/{}/source.tar.gz", m.product, source_sha);
    let meta = BTreeMap::from([
        ("stado-source-commit".into(), commit.clone()),
        ("stado-source-sha256".into(), source_sha.clone()),
        ("stado-manifest-sha256".into(), manifest_sha.clone()),
    ]);
    immutable(&source_uri, &archive, "application/gzip", &meta).await?;
    release_catalog::publish_entry(
        pm,
        manifest_sha.clone(),
        Some(CatalogSourceIdentity {
            commit: commit.clone(),
            source_sha256: source_sha.clone(),
            source_uri: source_uri.clone(),
        }),
    )
    .await?;
    let id = identity(
        &m.product,
        &args.version,
        channel,
        &source_sha,
        &manifest_sha,
    );
    let source_input_path = run_path(&m.product, &id, "inputs/source.tar.gz");
    queue_immutable(&source_input_path, &archive).await?;
    let manifest_path = run_path(&m.product, &id, "manifest.json");
    let manifest_uri = run_uri(&m.product, &id, "manifest.json");
    queue_immutable(&manifest_path, &manifest_bytes).await?;
    let now = Utc::now().to_rfc3339();
    let run = load(&id).await?.unwrap_or(ReleaseRun {
        schema_version: 1,
        run_id: id.clone(),
        product: m.product.clone(),
        version: args.version.clone(),
        channel,
        source_commit: commit.clone(),
        source_sha256: source_sha.clone(),
        source_uri: source_uri.clone(),
        manifest_sha256: manifest_sha.clone(),
        manifest_uri: manifest_uri.clone(),
        state: ReleaseRunState::Submitting,
        platforms: BTreeMap::new(),
        deliveries: BTreeMap::new(),
        failure: None,
        created_at: now.clone(),
        updated_at: now,
    });
    if run.source_commit != commit
        || run.source_sha256 != source_sha
        || run.manifest_sha256 != manifest_sha
    {
        return Err(CmdError::click("durable release run identity mismatch"));
    }
    if run.state == ReleaseRunState::Submitting {
        crate::cli::release_submit::changes::bind(&root, &commit, &id, &m.product).await?;
    }
    // Submitting is queueing. The builds run in the fleet, and the control
    // host's release agent signs, publishes and delivers when they are done;
    // the operator's terminal is not the place to wait an hour for a builder.
    continue_run(run, m, args.json, false).await
}
