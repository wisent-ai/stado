//! `stado build submit`: from a committed tree to queued platform jobs, and
//! the pieces of that walk a release submission shares.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cli::build_cmd::BuildSubmitArgs;
use crate::cli::release_catalog;
use crate::cli::release_submit::{committed_file, immutable, resolve_commit, snapshot};
use crate::cli::CmdError;
use crate::release_control;
use crate::release_pipeline::{
    self, CatalogSourceIdentity, ProductManifest, ReleasePipelineManifest, PRODUCT_MANIFEST,
};

use super::record::ensure_build;

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
    // A release carries the entries written for it: a repository that keeps
    // released entries in changelog/ moves them out of Unreleased in the
    // version-bump commit, so the submitted revision holds none there.
    if let Some(entries) = committed_file(&root, &commit, stado_product::changelog::CHANGELOG)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| stado_product::changelog::unreleased_entries(&text))
        .filter(|entries| !entries.is_empty())
    {
        return Err(CmdError::click(format!(
            "CHANGELOG.md at {commit} still holds {} Unreleased entries for {version}; run \
             'stado product changelog --version {version}' in the version-bump commit and submit that commit",
            entries.lines().filter(|line| line.starts_with("- ")).count()
        )));
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

/// Snapshot the committed tree and name the objects it will become. Git
/// only: nothing is written, so a build can be recorded — and a refusal
/// written on it — before anything the fleet must set up has been asked for.
pub(crate) fn snapshot_source(reading: &SourceReading) -> Result<StagedSource, CmdError> {
    let snapshot_phase = super::timing::phase("snapshot the committed tree");
    let archive = snapshot(&reading.root, &reading.commit)?;
    drop(snapshot_phase);
    let source_sha256 = release_control::sha256_bytes(&archive);
    let manifest_sha256 = release_control::sha256_bytes(&reading.manifest_bytes);
    let source_uri = format!(
        "stado://sources/{}/{}/source.tar.gz",
        reading.manifest.product, source_sha256
    );
    Ok(StagedSource {
        archive,
        source_sha256,
        manifest_sha256,
        source_uri,
    })
}

/// Enroll the product, publish the snapshot as the create-only source object
/// and record the manifest and source identity in the product catalog.
pub(crate) async fn publish_source(
    reading: &SourceReading,
    staged: &StagedSource,
) -> Result<(), CmdError> {
    // Whatever the manifest needs from the fleet is set up before the first
    // write: the source object is written with the product's own publisher
    // bearer, and its jobs read the build secrets the manifest names. A
    // required platform without post-build tests still builds, and is named
    // here because none of its builds can qualify a task.
    {
        let _phase = super::timing::phase("enroll the product in the fleet");
        release_catalog::missing_programs_refusal(
            &reading.manifest.product,
            &release_catalog::missing_step_programs(&reading.manifest, &reading.root),
        )?;
        let enrollment = release_catalog::enroll(&reading.manifest).await?;
        if let Err(finding) =
            release_catalog::untested_refusal(&reading.manifest.product, &enrollment.untested)
        {
            eprintln!("warning: {finding}");
        }
    }
    let meta = BTreeMap::from([
        ("stado-source-commit".into(), reading.commit.clone()),
        ("stado-source-sha256".into(), staged.source_sha256.clone()),
        (
            "stado-manifest-sha256".into(),
            staged.manifest_sha256.clone(),
        ),
    ]);
    {
        let _phase = super::timing::phase(format!(
            "upload the {} byte source archive",
            staged.archive.len()
        ));
        immutable(
            &staged.source_uri,
            &staged.archive,
            "application/gzip",
            &meta,
        )
        .await?;
    }
    let _phase = super::timing::phase("record the source in the product catalog");
    release_catalog::publish_entry(
        reading.product.clone(),
        staged.manifest_sha256.clone(),
        Some(CatalogSourceIdentity {
            commit: reading.commit.clone(),
            source_sha256: staged.source_sha256.clone(),
            source_uri: staged.source_uri.clone(),
        }),
    )
    .await?;
    Ok(())
}

/// The snapshot, published: what a release run consumes before it records
/// its build.
pub(crate) async fn stage_source(reading: &SourceReading) -> Result<StagedSource, CmdError> {
    let staged = snapshot_source(reading)?;
    publish_source(reading, &staged).await?;
    Ok(staged)
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
    let staged = snapshot_source(&reading)?;
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
