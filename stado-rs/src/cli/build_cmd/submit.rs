//! `stado build submit`: from a committed tree to queued platform jobs, and
//! the pieces of that walk a release submission shares.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::cli::build_cmd::BuildSubmitArgs;
use crate::cli::release_catalog;
use crate::cli::release_submit::{
    build_identity, build_path, build_uri, committed_file, enqueue_platforms, immutable,
    load_build, persist_build_failure, queue_immutable, refresh_build, resolve_commit, save_build,
    snapshot,
};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_control;
use crate::release_pipeline::{
    self, BuildRun, BuildRunState, CatalogSourceIdentity, ProductManifest, ReleasePipelineManifest,
    PRODUCT_MANIFEST,
};

const OBJECT_API_CATALOG_SERVICE: &str = "stado";
const OBJECT_API_REASON: &str =
    "build submission requires the canonical object store before its first write";

/// A committed tree read for building: nothing has been written anywhere.
pub(crate) struct SourceReading {
    pub root: PathBuf,
    pub commit: String,
    pub manifest_bytes: Vec<u8>,
    pub product: ProductManifest,
    pub manifest: ReleasePipelineManifest,
}

/// The same tree as immutable objects the fleet can read.
pub(crate) struct StagedSource {
    pub archive: Vec<u8>,
    pub source_sha256: String,
    pub manifest_sha256: String,
    pub source_uri: String,
}

/// Read the manifest and the declared version out of one commit, and refuse
/// a `--version` that disagrees with it. Git only; no store is touched.
pub(crate) fn read_source(
    source: &Path,
    commit: Option<&str>,
    version: &str,
) -> Result<SourceReading, CmdError> {
    let root = source.canonicalize()?;
    let commit = resolve_commit(&root, commit)?;
    let manifest_bytes = committed_file(&root, &commit, PRODUCT_MANIFEST)?;
    let product =
        release_pipeline::parse_product_manifest(&manifest_bytes).map_err(CmdError::click)?;
    let ProductManifest::Release(manifest) = product.clone() else {
        return Err(CmdError::click("product declares releases:false"));
    };
    let declared = release_pipeline::declared_version(&manifest.version_source, |path| {
        committed_file(&root, &commit, path).map_err(|error| error.to_string())
    })
    .map_err(CmdError::click)?;
    if declared != version {
        return Err(CmdError::click(
            "--version disagrees with declared version source",
        ));
    }
    Ok(SourceReading {
        root,
        commit,
        manifest_bytes,
        product,
        manifest,
    })
}

/// Make sure the object store this process would write to is there. An
/// explicit endpoint may be a Stado-managed loopback forward to the control
/// host; only an absent endpoint means this caller owns the local object
/// daemon and must ensure it before publishing.
pub(crate) async fn ensure_object_store() -> Result<(), CmdError> {
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
    Ok(())
}

/// Snapshot the committed tree, publish it as the create-only source object
/// and record the manifest and source identity in the product catalog.
pub(crate) async fn stage_source(reading: &SourceReading) -> Result<StagedSource, CmdError> {
    // Whatever the manifest needs from the fleet is set up before the first
    // write: the source object is written with the product's own publisher
    // bearer, and its jobs read the build secrets the manifest names. A
    // required platform without post-build tests still builds, and is named
    // here because none of its builds can qualify a task.
    {
        let _phase = super::timing::phase("enroll the product in the fleet");
        let enrollment = release_catalog::enroll(&reading.manifest).await?;
        if let Err(finding) =
            release_catalog::untested_refusal(&reading.manifest.product, &enrollment.untested)
        {
            eprintln!("warning: {finding}");
        }
    }
    let snapshot_phase = super::timing::phase("snapshot the committed tree");
    let archive = snapshot(&reading.root, &reading.commit)?;
    drop(snapshot_phase);
    let source_sha256 = release_control::sha256_bytes(&archive);
    let manifest_sha256 = release_control::sha256_bytes(&reading.manifest_bytes);
    let source_uri = format!(
        "stado://sources/{}/{}/source.tar.gz",
        reading.manifest.product, source_sha256
    );
    let meta = BTreeMap::from([
        ("stado-source-commit".into(), reading.commit.clone()),
        ("stado-source-sha256".into(), source_sha256.clone()),
        ("stado-manifest-sha256".into(), manifest_sha256.clone()),
    ]);
    {
        let _phase =
            super::timing::phase(format!("upload the {} byte source archive", archive.len()));
        immutable(&source_uri, &archive, "application/gzip", &meta).await?;
    }
    let _phase = super::timing::phase("record the source in the product catalog");
    release_catalog::publish_entry(
        reading.product.clone(),
        manifest_sha256.clone(),
        Some(CatalogSourceIdentity {
            commit: reading.commit.clone(),
            source_sha256: source_sha256.clone(),
            source_uri: source_uri.clone(),
        }),
    )
    .await?;
    Ok(StagedSource {
        archive,
        source_sha256,
        manifest_sha256,
        source_uri,
    })
}

/// The record of this staged source's build: created if it is new, loaded
/// if it is not, its inputs staged where its jobs read them. Nothing is
/// queued here; `queue_build` does that, and a release run does it through
/// its own continuation.
pub(crate) async fn record_build(
    reading: &SourceReading,
    staged: &StagedSource,
    version: &str,
) -> Result<BuildRun, CmdError> {
    let m = &reading.manifest;
    let id = build_identity(
        &m.product,
        version,
        &staged.source_sha256,
        &staged.manifest_sha256,
    );
    {
        let _phase = super::timing::phase("stage the build inputs in the queue");
        queue_immutable(
            &build_path(&m.product, &id, "inputs/source.tar.gz"),
            &staged.archive,
        )
        .await?;
        queue_immutable(
            &build_path(&m.product, &id, "manifest.json"),
            &reading.manifest_bytes,
        )
        .await?;
    }
    let now = Utc::now().to_rfc3339();
    let mut build = load_build(&id).await?.unwrap_or(BuildRun {
        schema_version: 1,
        build_id: id.clone(),
        product: m.product.clone(),
        version: version.to_owned(),
        source_commit: reading.commit.clone(),
        source_sha256: staged.source_sha256.clone(),
        source_uri: staged.source_uri.clone(),
        manifest_sha256: staged.manifest_sha256.clone(),
        manifest_uri: build_uri(&m.product, &id, "manifest.json"),
        state: BuildRunState::Waiting,
        platforms: BTreeMap::new(),
        failure: None,
        created_at: now.clone(),
        updated_at: now,
    });
    if build.source_commit != reading.commit
        || build.source_sha256 != staged.source_sha256
        || build.manifest_sha256 != staged.manifest_sha256
    {
        return Err(CmdError::click("durable build identity mismatch"));
    }
    {
        let _phase = super::timing::phase("bind the commit to its release batch");
        crate::cli::release_submit::changes::bind(&reading.root, &reading.commit, &id, &m.product)
            .await?;
    }
    build.failure = None;
    save_build(&mut build).await?;
    Ok(build)
}

/// Queue every platform the build still owes and read what its jobs did.
/// The enqueue failure that stopped the walk, if one did, comes back so the
/// caller can say what was queued before reporting it; a build that queued
/// nothing records that failure as its own and it is returned as the error.
pub(crate) async fn queue_build(
    build: &mut BuildRun,
    m: &ReleasePipelineManifest,
) -> Result<Option<CmdError>, CmdError> {
    let store = match JobStorage::new().await {
        Ok(store) => store,
        Err(error) => {
            return Err(persist_build_failure(build, CmdError::click(error.to_string())).await)
        }
    };
    let platforms: Vec<_> = m.platforms.keys().cloned().collect();
    let enqueue_phase = super::timing::phase("queue the platform jobs");
    let mut enqueue_failure = enqueue_platforms(&store, build, m, &platforms).await?;
    drop(enqueue_phase);
    let queued_nothing = build
        .platforms
        .values()
        .all(|platform| platform.state == release_pipeline::PlatformRunState::Failed);
    if queued_nothing {
        if let Some(error) = enqueue_failure.take() {
            return Err(persist_build_failure(build, error).await);
        }
    }
    let _phase = super::timing::phase("read what the queued jobs did");
    refresh_build(&store, build, m).await?;
    if let Some(error) = &enqueue_failure {
        build.failure = Some(format!("not every platform was queued: {error}"));
    }
    save_build(build).await?;
    Ok(enqueue_failure)
}

/// The build of this staged source, recorded and queued.
pub(crate) async fn ensure_build(
    reading: &SourceReading,
    staged: &StagedSource,
    version: &str,
) -> Result<(BuildRun, Option<CmdError>), CmdError> {
    let mut build = record_build(reading, staged, version).await?;
    let enqueue_failure = queue_build(&mut build, &reading.manifest).await?;
    Ok((build, enqueue_failure))
}

pub(super) async fn submit(args: &BuildSubmitArgs) -> Result<(), CmdError> {
    let reading = {
        let _phase = super::timing::phase("read the committed manifest and version");
        read_source(&args.source, args.commit.as_deref(), &args.version)?
    };
    {
        let _phase = super::timing::phase("ensure the object store");
        ensure_object_store().await?;
    }
    let staged = stage_source(&reading).await?;
    let (build, enqueue_failure) = ensure_build(&reading, &staged, &args.version).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&build)?)
    } else {
        println!(
            "build {} product={} version={} commit={} state={}: {}; `stado build status {}` follows it",
            build.build_id,
            build.product,
            build.version,
            build.source_commit,
            build.state.word(),
            super::report::summary(&build),
            build.build_id
        )
    }
    if let Some(error) = enqueue_failure {
        return Err(CmdError::click(format!(
            "build {} is waiting on the platforms it could queue, but one was refused: {error}; repeating `stado build submit` retries it",
            build.build_id
        )));
    }
    Ok(())
}
