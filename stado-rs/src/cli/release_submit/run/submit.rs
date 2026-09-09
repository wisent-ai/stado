//! `stado release submit` — the coordinator that walks one source tree
//! through every stage of the release pipeline.

use std::collections::BTreeMap;

use chrono::Utc;

use crate::cli::release_catalog;
use crate::cli::release_cmd;
use crate::cli::release_submit::builds::jobs::platforms::enqueue_platforms;
use crate::cli::release_submit::deliver::deliveries::run_deliveries;
use crate::cli::release_submit::publish::artifact::publish;
use crate::cli::release_submit::publish::promotion::reconcile;
use crate::cli::release_submit::publish::signing::{require_rollback_compatibility, signing};
use crate::cli::release_submit::run::source::{
    identity, immutable, queue_immutable, run_path, run_uri, snapshot,
};
use crate::cli::release_submit::run::state::{load, persist_failure, save};
use crate::cli::release_submit::ReleaseSubmitArgs;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{
    self, CatalogSourceIdentity, PlatformRunState, ProductManifest, ReleaseRun, ReleaseRunState,
    PRODUCT_MANIFEST,
};

const OBJECT_API_CATALOG_SERVICE: &str = "stado";
const OBJECT_API_REASON: &str =
    "release submission requires the canonical object store before its first write";

pub async fn submit(args: &ReleaseSubmitArgs) -> Result<(), CmdError> {
    let root = args.source.canonicalize()?;
    let manifest_bytes = std::fs::read(root.join(PRODUCT_MANIFEST))?;
    let pm = release_pipeline::parse_product_manifest(&manifest_bytes).map_err(CmdError::click)?;
    let ProductManifest::Release(m) = pm.clone() else {
        return Err(CmdError::click("product declares releases:false"));
    };
    let declared =
        release_pipeline::declared_version(&root, &m.version_source).map_err(CmdError::click)?;
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
    let (commit, archive) = snapshot(&root)?;
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
    let source_input_uri = run_uri(&m.product, &id, "inputs/source.tar.gz");
    queue_immutable(&source_input_path, &archive).await?;
    let manifest_path = run_path(&m.product, &id, "manifest.json");
    let manifest_uri = run_uri(&m.product, &id, "manifest.json");
    queue_immutable(&manifest_path, &manifest_bytes).await?;
    let now = Utc::now().to_rfc3339();
    let mut run = load(&id).await?.unwrap_or(ReleaseRun {
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
    run.failure = None;
    save(&mut run).await?;
    let store = match JobStorage::new().await {
        Ok(store) => store,
        Err(error) => {
            return Err(persist_failure(&mut run, CmdError::click(error.to_string())).await)
        }
    };
    let platforms: Vec<_> = m.platforms.keys().cloned().collect();
    let mut enqueue_failure = enqueue_platforms(
        &store,
        &mut run,
        &m,
        &args.version,
        &id,
        &commit,
        &source_sha,
        &source_input_uri,
        &manifest_sha,
        &manifest_uri,
        &platforms,
    )
    .await?;
    // A platform still recorded as failed at this point was not re-enqueued:
    // its own enqueue failed, or the loop stopped at an earlier platform.
    // Publishing it would only re-read the terminal job of the previous
    // attempt and report that attempt's failure again, ahead of the enqueue
    // error that is the actual diagnosis. Measured on weles-worker 0.5.72 on
    // 2026-09-05: four resumes each reported the same upload timeout from a
    // job that had finished an hour earlier, while the reason nothing new was
    // built - no eligible builder, or a store timeout while staging inputs -
    // was never printed. Only platforms this run actually submitted are
    // published; the enqueue error is returned below.
    let submitted_platforms: Vec<_> = platforms
        .iter()
        .filter(|platform| {
            run.platforms
                .get(*platform)
                .is_some_and(|record| record.state != PlatformRunState::Failed)
        })
        .cloned()
        .collect();
    // Nothing was submitted and the walk stopped: that failure IS the
    // diagnosis, so it is reported before this run reaches for signing
    // material. Reaching for it first replaces the reason nothing was built
    // with an unrelated one -- on a machine without the release signing
    // grant, `no live fleet builder can CLAIM release_platform ...` became
    // `cannot read signing key ...` in both the operator's error and the
    // durable run document, which is the same defect the comment above
    // describes, one stage later.
    if submitted_platforms.is_empty() {
        if let Some(error) = enqueue_failure.take() {
            return Err(persist_failure(&mut run, error).await);
        }
    }
    run.state = ReleaseRunState::Waiting;
    save(&mut run).await?;
    let (key, private) = match signing(&run.product).await {
        Ok(signing) => signing,
        Err(error) => return Err(persist_failure(&mut run, error).await),
    };
    run.state = ReleaseRunState::Publishing;
    save(&mut run).await?;
    let mut artifacts = BTreeMap::new();
    for p in &submitted_platforms {
        let result = if run.platforms[p].state == PlatformRunState::Published {
            release_cmd::verified_artifact_for_submit(&run.product, &run.version, p).await
        } else {
            publish(&mut run, &m, p, &store, &key, &private).await
        };
        let a = match result {
            Ok(artifact) => artifact,
            Err(error) => {
                let platform = run.platforms.get_mut(p).unwrap();
                platform.state = PlatformRunState::Failed;
                platform.failure = Some(error.to_string());
                if m.platforms[p].required {
                    return Err(persist_failure(&mut run, error).await);
                }
                save(&mut run).await?;
                continue;
            }
        };
        save(&mut run).await?;
        artifacts.insert(p.clone(), a);
    }
    if let Some(error) = enqueue_failure {
        return Err(persist_failure(&mut run, error).await);
    }
    run.state = ReleaseRunState::Delivering;
    save(&mut run).await?;
    if let Err(error) = run_deliveries(&mut run, &m, &artifacts).await {
        return Err(persist_failure(&mut run, error).await);
    }
    if m.promotion.reconcile {
        if let Err(error) =
            release_cmd::promote_for_submit(&run.product, &run.version, run.channel).await
        {
            return Err(persist_failure(&mut run, error).await);
        }
        if let Err(error) = reconcile(&run).await {
            return Err(persist_failure(&mut run, error).await);
        }
        run.state = ReleaseRunState::Reconciled
    } else {
        run.state = ReleaseRunState::Completed
    }
    save(&mut run).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&run)?)
    } else {
        println!(
            "release run {} product={} version={} state={:?}",
            run.run_id, run.product, run.version, run.state
        )
    }
    Ok(())
}
