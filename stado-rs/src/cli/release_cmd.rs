//! `stado release ...` — signed immutable product release publication,
//! promotion, host reconciliation, status, and rollback.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::Utc;
use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::release_control::{
    self, DesiredRelease, ProductReleasePolicy, QualificationStatus, ReleaseArtifactRef,
    ReleaseChannel, ReleaseControl, ReleaseManifest, ReleaseQualification,
};
use crate::release_pipeline::{BuildReceipt, StepStatus};

use super::CmdError;

#[derive(Subcommand)]
pub enum ReleaseCommands {
    /// Generate an Ed25519 release authority key pair.
    Keygen(ReleaseKeygenArgs),
    /// Snapshot, qualify, build, sign, publish, deliver, and promote a product.
    Submit(crate::cli::release_submit::ReleaseSubmitArgs),
    /// Manage the Stado-owned product and source policy catalog.
    Catalog(crate::cli::release_catalog::CatalogArgs),
    /// Internal provider-neutral release build worker.
    #[command(hide = true)]
    Worker(crate::cli::release_submit::ReleaseWorkerArgs),
    /// Internal provider-neutral post-publication delivery worker.
    #[command(name = "delivery-worker", hide = true)]
    DeliveryWorker(crate::cli::release_submit::DeliveryWorkerArgs),
    /// Build, sign, and publish one immutable candidate coordinate.
    Prepare(ReleasePrepareArgs),
    /// Promote exact qualified candidate bytes into registry desired state.
    Promote(ReleasePromoteArgs),
    /// Reconcile desired releases on this exact registry target.
    Agent(ReleaseAgentArgs),
    /// Internal stable-port proxy owned by the release agent.
    #[command(hide = true)]
    Proxy(ReleaseProxyArgs),
    /// Show desired and observed rollout state.
    Status(ReleaseStatusArgs),
    /// Atomically restore the previous desired release.
    Rollback(ReleaseRollbackArgs),
}

#[derive(Args)]
pub struct ReleaseKeygenArgs {
    #[arg(long)]
    private_key: PathBuf,
    #[arg(long)]
    public_key: PathBuf,
    #[arg(long)]
    key_id: String,
}

#[derive(Args)]
pub struct ReleasePrepareArgs {
    pub product: String,
    pub version: String,
    pub platform: String,
    #[arg(long)]
    archive: PathBuf,
    #[arg(long)]
    source_revision: String,
    #[arg(long)]
    binary: String,
    #[arg(long)]
    launcher: String,
    #[arg(long)]
    signing_key_item: String,
    #[arg(long)]
    key_id: String,
    #[arg(long)]
    source_sha256: String,
    #[arg(long)]
    pipeline_manifest_sha256: String,
    #[arg(long)]
    qualification: PathBuf,
    #[arg(long, default_value_t = 1)]
    config_schema: u64,
    #[arg(long, default_value_t = 1)]
    state_schema: u64,
    #[arg(long)]
    minimum_stado_version: String,
    #[arg(long = "rollback-compatible-with")]
    rollback_compatible_with: Vec<String>,
    #[arg(long, default_value = "unknown")]
    builder: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleasePromoteArgs {
    pub product: String,
    pub version: String,
    #[arg(long, value_enum, default_value_t = ChannelArg::Stable)]
    channel: ChannelArg,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ChannelArg {
    Candidate,
    Stable,
}

impl From<ChannelArg> for ReleaseChannel {
    fn from(channel: ChannelArg) -> Self {
        match channel {
            ChannelArg::Candidate => Self::Candidate,
            ChannelArg::Stable => Self::Stable,
        }
    }
}

#[derive(Args)]
pub struct ReleaseAgentArgs {
    #[arg(long)]
    target: String,
    #[arg(long)]
    once: bool,
    #[arg(long, default_value_t = 15)]
    interval_seconds: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleaseProxyArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    bind: String,
}

#[derive(Args)]
pub struct ReleaseStatusArgs {
    pub product: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleaseRollbackArgs {
    pub product: String,
    #[arg(long)]
    json: bool,
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CmdError> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", path.display())))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_public(path: &Path, bytes: &[u8]) -> Result<(), CmdError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", path.display())))?;
    writeln!(file, "{}", BASE64.encode(bytes))?;
    file.sync_all()?;
    Ok(())
}

async fn keygen(args: &ReleaseKeygenArgs) -> Result<(), CmdError> {
    if args.key_id.is_empty() {
        return Err(CmdError::usage("--key-id must not be empty"));
    }
    let (private, public) = release_control::generate_signing_key().map_err(CmdError::click)?;
    write_private(&args.private_key, &private)?;
    if let Err(error) = write_public(&args.public_key, &public) {
        let _ = std::fs::remove_file(&args.private_key);
        return Err(error);
    }
    println!(
        "generated release key {} private={} public={}",
        args.key_id,
        args.private_key.display(),
        args.public_key.display()
    );
    Ok(())
}

async fn put_immutable(uri: &str, bytes: &[u8], content_type: &str) -> Result<(), CmdError> {
    match crate::cli::storage::fetch_object(uri).await {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => {
            return Err(CmdError::click(format!(
                "immutable release object already differs: {uri}"
            )))
        }
        Err(_) => {}
    }
    let temporary = tempfile::NamedTempFile::new()?;
    std::fs::write(temporary.path(), bytes)?;
    crate::cli::storage::store_object(
        uri,
        &temporary.path().display().to_string(),
        content_type,
        true,
    )
    .await
    .map(|_| ())
}


pub(crate) struct PipelinePublishRequest<'a> {
    pub product: &'a str,
    pub version: &'a str,
    pub platform: &'a str,
    pub archive: &'a [u8],
    pub source_revision: &'a str,
    pub source_sha256: &'a str,
    pub pipeline_manifest_sha256: &'a str,
    pub binary: &'a str,
    pub launcher: &'a str,
    pub config_schema: u64,
    pub state_schema: u64,
    pub minimum_stado_version: &'a str,
    pub rollback_compatible_with: &'a [String],
    pub qualification: ReleaseQualification,
    pub qualification_receipt: &'a [u8],
    pub key_id: &'a str,
    pub private_key: &'a [u8],
    pub builder: &'a str,
}

pub(crate) async fn publish_pipeline_release(
    request: PipelinePublishRequest<'_>,
) -> Result<(ReleaseArtifactRef, ReleaseManifest), CmdError> {
    let artifact_bytes = request.archive.len() as u64;
    let artifact_sha256 = release_control::sha256_bytes(request.archive);
    let qualification_receipt_sha256 = release_control::sha256_bytes(request.qualification_receipt);
    if request.qualification.evidence_sha256.as_deref()
        != Some(qualification_receipt_sha256.as_str())
    {
        return Err(CmdError::click(
            "qualification evidence digest does not match its immutable receipt",
        ));
    }
    let manifest = ReleaseManifest {
        schema_version: 1,
        product: request.product.to_string(),
        version: request.version.to_string(),
        platform: request.platform.to_string(),
        source_revision: request.source_revision.to_string(),
        source_sha256: request.source_sha256.to_string(),
        pipeline_manifest_sha256: request.pipeline_manifest_sha256.to_string(),
        qualification_receipt_sha256,
        artifact_sha256,
        artifact_bytes,
        binary: request.binary.to_string(),
        launcher: request.launcher.to_string(),
        config_schema: request.config_schema,
        state_schema: request.state_schema,
        minimum_stado_version: request.minimum_stado_version.to_string(),
        rollback_compatible_with: request.rollback_compatible_with.to_vec(),
        qualification: request.qualification,
        key_id: request.key_id.to_string(),
        built_at: Utc::now().to_rfc3339(),
        builder: request.builder.to_string(),
    };
    release_control::validate_manifest(&manifest).map_err(CmdError::click)?;
    let manifest_bytes = release_control::canonical_manifest(&manifest).map_err(CmdError::click)?;
    let signature =
        release_control::sign_manifest(request.private_key, &manifest).map_err(CmdError::click)?;
    let base = release_control::release_base(request.product, request.version, request.platform)
        .map_err(CmdError::click)?;
    let archive_uri = format!("{base}/{}", release_control::RELEASE_ARCHIVE_NAME);
    let qualification_uri = format!("{base}/{}", release_control::RELEASE_QUALIFICATION_NAME);
    let signature_uri = format!("{base}/{}", release_control::RELEASE_SIGNATURE_NAME);
    let manifest_uri = format!("{base}/{}", release_control::RELEASE_MANIFEST_NAME);
    put_immutable(&archive_uri, request.archive, "application/gzip").await?;
    put_immutable(
        &qualification_uri,
        request.qualification_receipt,
        "application/json",
    )
    .await?;
    put_immutable(
        &signature_uri,
        format!("{signature}\n").as_bytes(),
        "text/plain",
    )
    .await?;
    // The signed manifest is the immutable commit marker and always lands last.
    put_immutable(&manifest_uri, &manifest_bytes, "application/json").await?;
    Ok((
        ReleaseArtifactRef {
            manifest_uri,
            signature_uri,
            archive_uri,
            manifest_sha256: release_control::sha256_bytes(&manifest_bytes),
            artifact_sha256: manifest.artifact_sha256.clone(),
            source_revision: manifest.source_revision.clone(),
            key_id: manifest.key_id.clone(),
        },
        manifest,
    ))
}

async fn signing_key(item: &str) -> Result<Vec<u8>, CmdError> {
    let encoded = crate::credential_store::read_string(item, "private_key")
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "Skarbiec item {item:?} field private_key is required"
            ))
        })?;
    BASE64
        .decode(encoded)
        .map_err(|_| CmdError::click("release signing key field is not base64"))
}

async fn prepare(args: &ReleasePrepareArgs) -> Result<(), CmdError> {
    let archive = std::fs::read(&args.archive)?;
    let qualification_receipt = std::fs::read(&args.qualification)?;
    let receipt: BuildReceipt = serde_json::from_slice(&qualification_receipt)?;
    let artifact_sha256 = release_control::sha256_bytes(&archive);
    if receipt.product != args.product
        || receipt.version != args.version
        || receipt.platform != args.platform
        || receipt.builder != args.builder
        || receipt.source_commit != args.source_revision
        || receipt.source_sha256 != args.source_sha256
        || receipt.manifest_sha256 != args.pipeline_manifest_sha256
        || receipt.status != StepStatus::Passed
        || receipt.artifact.as_ref().map(|value| value.sha256.as_str())
            != Some(artifact_sha256.as_str())
    {
        return Err(CmdError::click(
            "qualification receipt does not describe this prepared artifact",
        ));
    }
    let qualification = ReleaseQualification {
        status: QualificationStatus::Passed,
        evidence_sha256: Some(release_control::sha256_bytes(&qualification_receipt)),
        completed_at: Some(receipt.completed_at),
    };
    let private = signing_key(&args.signing_key_item).await?;
    let public = release_control::signing_public_key(&private).map_err(CmdError::click)?;
    let (artifact, manifest) = publish_pipeline_release(PipelinePublishRequest {
        product: &args.product,
        version: &args.version,
        platform: &args.platform,
        archive: &archive,
        source_revision: &args.source_revision,
        source_sha256: &args.source_sha256,
        pipeline_manifest_sha256: &args.pipeline_manifest_sha256,
        binary: &args.binary,
        launcher: &args.launcher,
        config_schema: args.config_schema,
        state_schema: args.state_schema,
        minimum_stado_version: &args.minimum_stado_version,
        rollback_compatible_with: &args.rollback_compatible_with,
        qualification,
        qualification_receipt: &qualification_receipt,
        key_id: &args.key_id,
        private_key: &private,
        builder: &args.builder,
    })
    .await?;
    let report = json!({
        "product": args.product,
        "version": args.version,
        "platform": args.platform,
        "source_revision": args.source_revision,
        "source_sha256": args.source_sha256,
        "pipeline_manifest_sha256": args.pipeline_manifest_sha256,
        "artifact_sha256": artifact.artifact_sha256,
        "manifest_sha256": artifact.manifest_sha256,
        "key_id": args.key_id,
        "public_key": BASE64.encode(public),
        "qualification": manifest.qualification.status,
        "archive_uri": artifact.archive_uri,
        "signature_uri": artifact.signature_uri,
        "manifest_uri": artifact.manifest_uri,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "prepared {} {} {} artifact={} manifest={} key={}",
            args.product,
            args.version,
            args.platform,
            report["artifact_sha256"].as_str().unwrap_or_default(),
            report["manifest_sha256"].as_str().unwrap_or_default(),
            args.key_id
        );
    }
    Ok(())
}

pub(crate) async fn verified_artifact(
    product: &str,
    version: &str,
    platform: &str,
    control: &ReleaseControl,
) -> Result<ReleaseArtifactRef, CmdError> {
    let base =
        release_control::release_base(product, version, platform).map_err(CmdError::click)?;
    let manifest_uri = format!("{base}/{}", release_control::RELEASE_MANIFEST_NAME);
    let signature_uri = format!("{base}/{}", release_control::RELEASE_SIGNATURE_NAME);
    let archive_uri = format!("{base}/{}", release_control::RELEASE_ARCHIVE_NAME);
    let manifest_bytes = crate::cli::storage::fetch_object(&manifest_uri).await?;
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes)?;
    release_control::validate_manifest(&manifest).map_err(CmdError::click)?;
    if manifest.product != product || manifest.version != version || manifest.platform != platform {
        return Err(CmdError::click(
            "release manifest identity does not match its object coordinate",
        ));
    }
    if manifest.qualification.status != QualificationStatus::Passed {
        return Err(CmdError::click(format!(
            "release {product} {version} {platform} has not passed qualification"
        )));
    }
    let public = control.trusted_keys.get(&manifest.key_id).ok_or_else(|| {
        CmdError::click(format!(
            "release key {} is not trusted by registry",
            manifest.key_id
        ))
    })?;
    let signature = crate::cli::storage::fetch_object(&signature_uri).await?;
    release_control::verify_manifest(
        public,
        &manifest,
        std::str::from_utf8(&signature)
            .map_err(|_| CmdError::click("release signature is not UTF-8"))?,
    )
    .map_err(CmdError::click)?;
    let archive = crate::cli::storage::fetch_object(&archive_uri).await?;
    if archive.len() as u64 != manifest.artifact_bytes
        || release_control::sha256_bytes(&archive) != manifest.artifact_sha256
    {
        return Err(CmdError::click(
            "release archive differs from its signed manifest",
        ));
    }
    Ok(ReleaseArtifactRef {
        manifest_uri,
        signature_uri,
        archive_uri,
        manifest_sha256: release_control::sha256_bytes(&manifest_bytes),
        artifact_sha256: manifest.artifact_sha256,
        source_revision: manifest.source_revision,
        key_id: manifest.key_id,
    })
}

pub(crate) async fn verified_artifact_for_submit(
    product: &str,
    version: &str,
    platform: &str,
) -> Result<ReleaseArtifactRef, CmdError> {
    let (document, _) = super::registry::fetch_versioned_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    verified_artifact(product, version, platform, &control).await
}

fn platforms(policy: &ProductReleasePolicy) -> BTreeSet<String> {
    policy
        .targets
        .values()
        .map(|target| target.platform.clone())
        .collect()
}

async fn promote(args: &ReleasePromoteArgs) -> Result<(), CmdError> {
    let (document, expected_generation) = super::registry::fetch_versioned_document().await?;
    let mut control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control
        .products
        .get(&args.product)
        .ok_or_else(|| CmdError::click(format!("unknown release product {:?}", args.product)))?;
    let mut artifacts = BTreeMap::new();
    let mut revisions = BTreeSet::new();
    for platform in platforms(policy) {
        let artifact = verified_artifact(&args.product, &args.version, &platform, &control).await?;
        revisions.insert(artifact.source_revision.clone());
        artifacts.insert(platform, artifact);
    }
    if revisions.len() != 1 {
        return Err(CmdError::click(
            "release platforms were not built from one source revision",
        ));
    }
    let policy = control
        .products
        .get_mut(&args.product)
        .expect("checked above");
    let rollout_generation = policy
        .desired
        .as_ref()
        .map_or(1, |desired| desired.rollout_generation.saturating_add(1));
    policy.previous = policy.desired.take();
    policy.desired = Some(DesiredRelease {
        version: args.version.clone(),
        channel: args.channel.into(),
        rollout_generation,
        promoted_at: Utc::now().to_rfc3339(),
        artifacts,
    });
    control.generation = control.generation.saturating_add(1);
    let mut updated = document;
    updated[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&control)?;
    let stored_generation =
        super::registry::push_document_if(&updated, &expected_generation).await?;
    let report = json!({
        "product": args.product,
        "version": args.version,
        "channel": ReleaseChannel::from(args.channel),
        "rollout_generation": rollout_generation,
        "registry_generation": control.generation,
        "store_generation": stored_generation,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "promoted {} {} channel={:?} rollout-generation={} registry-generation={}",
            args.product,
            args.version,
            ReleaseChannel::from(args.channel),
            rollout_generation,
            control.generation
        );
    }
    Ok(())
}

pub(crate) async fn promote_for_submit(
    product: &str,
    version: &str,
    channel: crate::release_pipeline::PipelineChannel,
) -> Result<(), CmdError> {
    promote(&ReleasePromoteArgs {
        product: product.to_string(),
        version: version.to_string(),
        channel: match channel {
            crate::release_pipeline::PipelineChannel::Candidate => ChannelArg::Candidate,
            crate::release_pipeline::PipelineChannel::Stable => ChannelArg::Stable,
        },
        json: false,
    })
    .await
}

async fn agent(args: &ReleaseAgentArgs) -> Result<(), CmdError> {
    let states = if args.once {
        crate::release_agent::reconcile_once(&args.target)
            .await
            .map_err(CmdError::click)?
    } else {
        return crate::release_agent::agent(&args.target, false, args.interval_seconds)
            .await
            .map_err(CmdError::click);
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&states)?);
    } else {
        for state in states {
            println!(
                "{} target={} generation={} phase={:?} active={} detail={}",
                state.product,
                state.target,
                state.rollout_generation,
                state.phase,
                state
                    .active
                    .as_ref()
                    .map(|record| record.version.as_str())
                    .unwrap_or("-"),
                state.detail
            );
        }
    }
    Ok(())
}

async fn status(args: &ReleaseStatusArgs) -> Result<(), CmdError> {
    let document = super::registry::fetch_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let mut reports = Vec::new();
    for (product, policy) in &control.products {
        if args
            .product
            .as_deref()
            .is_some_and(|selected| selected != product)
        {
            continue;
        }
        for target in policy.targets.keys() {
            let uri = format!("stado://system/release-status/{product}/{target}.json");
            let observed: Value = match crate::cli::storage::fetch_object(&uri).await {
                Ok(bytes) => serde_json::from_slice(&bytes)?,
                Err(_) => Value::Null,
            };
            reports.push(json!({
                "product": product,
                "target": target,
                "desired": policy.desired,
                "previous": policy.previous,
                "observed": observed,
            }));
        }
    }
    if reports.is_empty() {
        return Err(CmdError::click("no matching release product"));
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for report in reports {
            println!(
                "{} target={} desired={} observed={}",
                report["product"].as_str().unwrap_or_default(),
                report["target"].as_str().unwrap_or_default(),
                report["desired"]["version"].as_str().unwrap_or("-"),
                report["observed"]["phase"].as_str().unwrap_or("unreported")
            );
        }
    }
    Ok(())
}

async fn rollback(args: &ReleaseRollbackArgs) -> Result<(), CmdError> {
    let (document, expected_generation) = super::registry::fetch_versioned_document().await?;
    let mut control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control
        .products
        .get_mut(&args.product)
        .ok_or_else(|| CmdError::click(format!("unknown release product {:?}", args.product)))?;
    let previous = policy
        .previous
        .take()
        .ok_or_else(|| CmdError::click("release has no previous desired version to restore"))?;
    let current = policy.desired.replace(previous);
    policy.previous = current;
    let desired = policy.desired.as_mut().expect("previous installed above");
    desired.rollout_generation =
        policy
            .previous
            .as_ref()
            .map_or(desired.rollout_generation.saturating_add(1), |release| {
                release
                    .rollout_generation
                    .max(desired.rollout_generation)
                    .saturating_add(1)
            });
    desired.promoted_at = Utc::now().to_rfc3339();
    let version = desired.version.clone();
    let rollout_generation = desired.rollout_generation;
    control.generation = control.generation.saturating_add(1);
    let mut updated = document;
    updated[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&control)?;
    let stored_generation =
        super::registry::push_document_if(&updated, &expected_generation).await?;
    let report = json!({
        "product": args.product,
        "version": version,
        "rollout_generation": rollout_generation,
        "registry_generation": control.generation,
        "store_generation": stored_generation,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "rollback requested product={} version={} rollout-generation={}",
            args.product, version, rollout_generation
        );
    }
    Ok(())
}

pub async fn dispatch(command: ReleaseCommands) -> Result<(), CmdError> {
    match command {
        ReleaseCommands::Keygen(args) => keygen(&args).await,
        ReleaseCommands::Submit(args) => crate::cli::release_submit::submit(&args).await,
        ReleaseCommands::Catalog(args) => crate::cli::release_catalog::dispatch(args).await,
        ReleaseCommands::Worker(args) => crate::cli::release_submit::worker(&args).await,
        ReleaseCommands::DeliveryWorker(args) => {
            crate::cli::release_submit::delivery_worker(&args).await
        }
        ReleaseCommands::Prepare(args) => prepare(&args).await,
        ReleaseCommands::Promote(args) => promote(&args).await,
        ReleaseCommands::Agent(args) => agent(&args).await,
        ReleaseCommands::Proxy(args) => crate::release_agent::proxy(&args.state, &args.bind)
            .await
            .map_err(CmdError::click),
        ReleaseCommands::Status(args) => status(&args).await,
        ReleaseCommands::Rollback(args) => rollback(&args).await,
    }
}
