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
        release_pipeline::parse_product_manifest(&manifest_bytes).map_err(CmdError::declaration)?;
    let ProductManifest::Release(manifest) = product.clone() else {
        return Err(CmdError::refused("product declares releases:false"));
    };
    let declared = release_pipeline::declared_version(&manifest.version_source, |path| {
        committed_file(&root, &commit, path).map_err(|error| error.to_string())
    })
    .map_err(CmdError::declaration)?;
    if declared != version {
        return Err(CmdError::refused(
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
        return Err(CmdError::refused(format!(
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
            let mut wrapped = CmdError::click(format!(
                "cannot ensure required service {OBJECT_API_CATALOG_SERVICE}: {error}"
            ));
            wrapped.failure = error.failure;
            wrapped
        })?;
    }
    Ok(())
}

/// Snapshot the committed tree and name the objects it will become. Git
/// only: nothing is written, so a build can be recorded — and a refusal
/// written on it — before anything the fleet must set up has been asked for.
pub(crate) fn snapshot_source(reading: &SourceReading) -> Result<StagedSource, CmdError> {
    let archive = crate::wait::blocking(
        crate::wait::Kind::Process,
        format!("git archive of commit {}", reading.commit),
        reading.root.display(),
        || snapshot(&reading.root, &reading.commit),
    )?;
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
    enroll_source(reading).await?;
    upload_source(reading, staged).await
}

/// Set up whatever the manifest needs from the fleet before the first write:
/// the source object is written with the product's own publisher bearer, and
/// its jobs read the build secrets the manifest names. It claims no version,
/// so a release submission runs it before binding the version to a commit.
pub(crate) async fn enroll_source(reading: &SourceReading) -> Result<(), CmdError> {
    release_catalog::missing_programs_refusal(
        &reading.manifest.product,
        &release_catalog::missing_step_programs(&reading.manifest, &reading.root),
    )?;
    release_catalog::enroll(&reading.manifest).await?;
    Ok(())
}

/// Publish the snapshot as the create-only source object and record the
/// manifest and source identity in the product catalog.
pub(crate) async fn upload_source(
    reading: &SourceReading,
    staged: &StagedSource,
) -> Result<(), CmdError> {
    let meta = BTreeMap::from([
        ("stado-source-commit".into(), reading.commit.clone()),
        ("stado-source-sha256".into(), staged.source_sha256.clone()),
        (
            "stado-manifest-sha256".into(),
            staged.manifest_sha256.clone(),
        ),
    ]);
    immutable(
        &staged.source_uri,
        &staged.archive,
        "application/gzip",
        &meta,
    )
    .await?;
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

/// The same reading narrowed to `platforms`: the manifest keeps only those
/// platforms and the deliveries that run on them, and is what the build
/// records, so its jobs, its enrollment and any release of it cover exactly
/// these platforms. A platform the manifest does not declare, or a kept
/// delivery that waits on a dropped one, is refused before anything is written.
fn restrict_platforms(
    mut reading: SourceReading,
    platforms: &[String],
) -> Result<SourceReading, CmdError> {
    let declared: Vec<String> = reading.manifest.platforms.keys().cloned().collect();
    if let Some(unknown) = platforms
        .iter()
        .find(|platform| !declared.contains(platform))
    {
        return Err(CmdError::usage(format!(
            "{} declares no platform {unknown}; declared: {}",
            reading.manifest.product,
            declared.join(", ")
        )));
    }
    let manifest = &mut reading.manifest;
    manifest
        .platforms
        .retain(|platform, _| platforms.contains(platform));
    let dropped: Vec<String> = manifest
        .deliveries
        .iter()
        .filter(|delivery| !platforms.contains(&delivery.platform))
        .map(|delivery| delivery.name.clone())
        .collect();
    manifest
        .deliveries
        .retain(|delivery| platforms.contains(&delivery.platform));
    if let Some((delivery, waits_on)) = manifest.deliveries.iter().find_map(|delivery| {
        delivery
            .after
            .iter()
            .find(|name| dropped.contains(name))
            .map(|name| (delivery.name.clone(), name.clone()))
    }) {
        return Err(CmdError::usage(format!(
            "delivery {delivery} waits on {waits_on}, which runs on a platform this build leaves out; build that platform too"
        )));
    }
    reading.manifest_bytes = serde_json::to_vec_pretty(&reading.manifest)?;
    reading.product = ProductManifest::Release(reading.manifest.clone());
    Ok(reading)
}

pub(super) async fn submit(args: &BuildSubmitArgs) -> Result<(), CmdError> {
    let reading = crate::wait::blocking(
        crate::wait::Kind::Process,
        "git read of the committed manifest and version",
        args.source.display(),
        || read_source(&args.source, args.commit.as_deref(), &args.version),
    )?;
    let reading = if args.platforms.is_empty() {
        reading
    } else {
        restrict_platforms(reading, &args.platforms)?
    };
    ensure_object_store().await?;
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
        // The queue's refusal already carries its class; the retry advice
        // must not replace it.
        let mut wrapped = CmdError::click(format!(
            "build {} is waiting on the platforms it could queue, but one was refused: {error}; repeating `stado build submit` retries it",
            build.build_id
        ));
        wrapped.failure = error.failure;
        return Err(wrapped);
    }
    Ok(())
}
