//! `stado release ...` — signed immutable product release publication,
//! promotion, host reconciliation, status, and rollback.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::Utc;
use clap::{Args, Subcommand};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::release_control::{
    self, DesiredRelease, ProductReleasePolicy, QualificationStatus, ReleaseArtifactRef,
    ReleaseChannel, ReleaseControl, ReleaseManifest, ReleaseQualification,
};
use crate::release_pipeline::{BuildReceipt, StepStatus};

use super::CmdError;

// A clap subcommand enum is constructed once per process from parsed argv, so
// the largest variant costs one stack frame at startup and boxing it would only
// add an allocation and a deref to every match arm.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum ReleaseCommands {
    /// Generate an Ed25519 release authority key pair.
    Keygen(ReleaseKeygenArgs),
    /// Apply reviewed product policy without changing the active release.
    #[command(name = "policy-apply")]
    PolicyApply(ReleasePolicyApplyArgs),
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
    /// Read a release candidate's own stdout/stderr off the target host.
    Logs(crate::cli::release_evidence::ReleaseLogsArgs),
    /// One verdict over desired state, the candidate, quarantine and the
    /// host's claiming gates.
    Doctor(crate::cli::release_evidence::ReleaseDoctorArgs),
    /// List and retire the digests a host refuses to roll out again.
    #[command(subcommand)]
    Quarantine(crate::cli::release_quarantine::QuarantineCommands),
    /// Atomically restore the previous desired release.
    Rollback(ReleaseRollbackArgs),
    /// Install a delivered release archive's binary on this very host.
    #[command(name = "install-local")]
    InstallLocal(ReleaseInstallLocalArgs),
    /// Bind one immutable coordinate to exactly one source revision before
    /// anything is published into it.
    #[command(name = "claim-coordinate")]
    ClaimCoordinate(ReleaseClaimCoordinateArgs),
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
pub struct ReleasePolicyApplyArgs {
    /// JSON document containing exactly `product` and `policy`.
    #[arg(long)]
    file: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasePolicyDocument {
    product: String,
    policy: ProductReleasePolicy,
}

/// `stado release install-local` — the delivery contract's local endpoint.
///
/// A delivery job pinned to its target runs ON that target, so installation
/// is a local file operation and needs no login service: the release that
/// installed over ssh died on the first host without Remote Login. The
/// archive path and digest come from the delivery worker's environment
/// (`WISENT_RELEASE_ARCHIVE`, `WISENT_RELEASE_SHA256`), the same contract
/// the retired python installer read. This command replaced the last
/// load-bearing script of the 137 deleted on 2026-08-19.
#[derive(Args)]
pub struct ReleaseInstallLocalArgs {
    /// Archive member to install, e.g. bin/stado.
    #[arg(long, default_value = "bin/stado")]
    member: String,
    /// Installed name under $HOME/.stado/bin; defaults to the member's
    /// basename.
    #[arg(long, default_value = "")]
    name: String,
}

/// `stado release claim-coordinate` — the publishers' shared first step.
///
/// Exposed as a command because two of the three publishers are workflow
/// steps: the tag train and the existing-release recovery run in bash, and a
/// rule re-implemented in bash is a second source of truth for the one thing
/// that decides whether a version means one build.
#[derive(Args)]
pub struct ReleaseClaimCoordinateArgs {
    pub product: String,
    pub version: String,
    pub platform: String,
    /// The exact commit these bytes were built from.
    #[arg(long)]
    source_commit: String,
    #[arg(long)]
    json: bool,
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
    #[arg(
        long,
        conflicts_with = "signing_key_file",
        required_unless_present = "signing_key_file"
    )]
    signing_key_item: Option<String>,
    #[arg(
        long,
        conflicts_with = "signing_key_item",
        required_unless_present = "signing_key_item"
    )]
    signing_key_file: Option<PathBuf>,
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
    product: Option<String>,
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
    match crate::cli::storage::fetch_object_from_writer(uri).await {
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

/// What claiming a coordinate found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoordinateClaim {
    /// This publisher wrote the coordinate's revision record.
    Claimed,
    /// The record already existed and names this same build.
    Confirmed,
}

impl CoordinateClaim {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Confirmed => "confirmed",
        }
    }
}

/// Bind one immutable coordinate to exactly one source revision, before any
/// artifact is written into it.
///
/// Every publisher calls this first: the signed pipeline through
/// [`publish_pipeline_release`], the tag train and the existing-release
/// recovery workflow through `stado release claim-coordinate`. The record is
/// created only if absent, so the first caller states the build and every
/// later caller either agrees — a republish of the same version from the same
/// commit, which is legitimate and idempotent — or is refused here, with
/// nothing published.
///
/// The refusal names both commits and says what to do, because the remedy is
/// not a retry: immutable objects mean a version that already attests one
/// build cannot be made to attest another.
pub(crate) async fn claim_release_coordinate(
    product: &str,
    version: &str,
    platform: &str,
    source_revision: &str,
) -> Result<CoordinateClaim, CmdError> {
    let claim =
        release_control::CoordinateRevision::new(product, version, platform, source_revision)
            .map_err(CmdError::click)?;
    let bytes = claim.canonical_bytes().map_err(CmdError::click)?;
    let base =
        release_control::release_base(product, version, platform).map_err(CmdError::click)?;
    let uri = format!("{base}/{}", release_control::RELEASE_REVISION_NAME);

    // A read that fails is not a coordinate without a record: the writer may
    // simply not have answered. That distinction is why nothing is decided on
    // a failed read here — the create-only put below is what decides, and it
    // fails loudly against a writer that cannot be reached.
    if let Ok(existing) = crate::cli::storage::fetch_object_from_writer(&uri).await {
        return judge_existing_claim(&claim, &existing, &uri);
    }
    // One version, one build, across platforms too.
    //
    // The record is per platform because that is where the artifacts live, and
    // on 2026-09-03 that turned out to be one scope too narrow: 0.14.3's linux
    // leg was claimed by the tag `8cf54ece` while another publisher had already
    // claimed the darwin leg from `ccc43c5e`, so the version was split across
    // producers with each platform internally consistent. A sibling that
    // attests a different commit is the same defect one level up, so it is
    // refused here — before this platform's first artifact — and named with
    // both commits.
    if let Some(conflict) = sibling_revision_conflict(&claim).await {
        return Err(conflict);
    }

    let temporary = tempfile::NamedTempFile::new()?;
    std::fs::write(temporary.path(), &bytes)?;
    match crate::cli::storage::store_object(
        &uri,
        &temporary.path().display().to_string(),
        "application/json",
        true,
    )
    .await
    {
        Ok(_) => Ok(CoordinateClaim::Claimed),
        Err(error) => {
            // Two publishers can reach this line at once, and the loser must
            // read the winner's record rather than report a write conflict:
            // one of them is about to publish artifacts and the other must
            // learn why it may not.
            match crate::cli::storage::fetch_object_from_writer(&uri).await {
                Ok(existing) => judge_existing_claim(&claim, &existing, &uri),
                Err(_) => Err(error),
            }
        }
    }
}

/// The first sibling platform of this version that attests a different commit.
///
/// `None` when every readable sibling agrees, and also when nothing can be
/// read: an unreachable store is not evidence of a second build, and the
/// per-platform record is still the claim that decides. This only ever adds a
/// refusal that names two commits, never a permission.
async fn sibling_revision_conflict(
    claim: &release_control::CoordinateRevision,
) -> Option<CmdError> {
    let coordinates = crate::cli::storage::published_release_coordinates(&claim.product)
        .await
        .ok()?;
    for (version, platform) in coordinates {
        if version != claim.version || platform == claim.platform {
            continue;
        }
        let base = release_control::release_base(&claim.product, &version, &platform).ok()?;
        let uri = format!("{base}/{}", release_control::RELEASE_REVISION_NAME);
        let Ok(bytes) = crate::cli::storage::fetch_object_from_writer(&uri).await else {
            continue;
        };
        let Ok(held) = serde_json::from_slice::<release_control::CoordinateRevision>(&bytes) else {
            continue;
        };
        if held.source_revision != claim.source_revision {
            return Some(CmdError::click(format!(
                "{}/{} already attests source revision {} for platform {}, and this publisher \
                 carries {} for {}. A version's platforms are one build: publish a new version \
                 rather than splitting this one across two commits",
                claim.product,
                claim.version,
                held.source_revision,
                platform,
                claim.source_revision,
                claim.platform
            )));
        }
    }
    None
}

fn judge_existing_claim(
    claim: &release_control::CoordinateRevision,
    existing: &[u8],
    uri: &str,
) -> Result<CoordinateClaim, CmdError> {
    let held: release_control::CoordinateRevision =
        serde_json::from_slice(existing).map_err(|error| {
            CmdError::click(format!(
                "{uri} is not a coordinate revision record: {error}"
            ))
        })?;
    if !held.describes(&claim.product, &claim.version, &claim.platform) {
        return Err(CmdError::click(format!(
            "{uri} attests {}/{}/{} and not {}/{}/{}",
            held.product, held.version, held.platform, claim.product, claim.version, claim.platform
        )));
    }
    if held.source_revision != claim.source_revision {
        return Err(CmdError::click(format!(
            "{}/{}/{} already attests source revision {}, and this publisher carries {}. \
             Release objects are immutable, so one version can never mean two builds: publish a \
             new version instead of writing a second build into this coordinate",
            claim.product,
            claim.version,
            claim.platform,
            held.source_revision,
            claim.source_revision
        )));
    }
    Ok(CoordinateClaim::Confirmed)
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
    // The coordinate's identity first, then its bytes. Publishing an archive
    // into a coordinate another build already owns is the failure this order
    // removes: the claim is refused while the prefix is still empty of
    // artifacts, instead of being detected at delivery once both producers
    // have written and the version is spent.
    claim_release_coordinate(
        request.product,
        request.version,
        request.platform,
        request.source_revision,
    )
    .await?;
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
    let private = match (&args.signing_key_item, &args.signing_key_file) {
        (Some(item), None) => signing_key(item).await?,
        (None, Some(path)) => {
            use std::os::unix::fs::PermissionsExt as _;
            let metadata = std::fs::metadata(path)?;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(CmdError::click(
                    "release signing key file must be owner-only",
                ));
            }
            std::fs::read(path)?
        }
        _ => {
            return Err(CmdError::usage(
                "prepare needs exactly one of --signing-key-item or --signing-key-file",
            ))
        }
    };
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

pub(crate) async fn verified_artifact_with_archive(
    product: &str,
    version: &str,
    platform: &str,
    control: &ReleaseControl,
) -> Result<(ReleaseArtifactRef, Vec<u8>), CmdError> {
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
    Ok((
        ReleaseArtifactRef {
            manifest_uri,
            signature_uri,
            archive_uri,
            manifest_sha256: release_control::sha256_bytes(&manifest_bytes),
            artifact_sha256: manifest.artifact_sha256,
            source_revision: manifest.source_revision,
            key_id: manifest.key_id,
        },
        archive,
    ))
}

pub(crate) async fn verified_artifact(
    product: &str,
    version: &str,
    platform: &str,
    control: &ReleaseControl,
) -> Result<ReleaseArtifactRef, CmdError> {
    verified_artifact_with_archive(product, version, platform, control)
        .await
        .map(|(artifact, _)| artifact)
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

async fn apply_policy(args: &ReleasePolicyApplyArgs) -> Result<(), CmdError> {
    let bytes = std::fs::read(&args.file)?;
    let mut declaration: ReleasePolicyDocument = serde_json::from_slice(&bytes)?;
    if declaration.policy.desired.is_some() || declaration.policy.previous.is_some() {
        return Err(CmdError::click(
            "rollout policy cannot set desired or previous release state; use release promote",
        ));
    }
    let (document, expected_generation) = super::registry::fetch_versioned_document().await?;
    let mut control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    if let Some(current) = control.products.get(&declaration.product) {
        declaration.policy.desired = current.desired.clone();
        declaration.policy.previous = current.previous.clone();
    }
    control
        .products
        .insert(declaration.product.clone(), declaration.policy);
    control.generation = control.generation.saturating_add(1);
    let mut updated = document;
    updated[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&control)?;
    release_control::validate_registry_contract(&updated).map_err(CmdError::click)?;
    let store_generation =
        super::registry::push_document_if(&updated, &expected_generation).await?;
    let report = json!({
        "product": declaration.product,
        "release_control_generation": control.generation,
        "store_generation": store_generation,
        "status": "applied",
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "applied rollout policy for {} at release-control generation {}",
            report["product"].as_str().unwrap_or_default(),
            control.generation
        );
    }
    Ok(())
}

async fn promote(args: &ReleasePromoteArgs, exact_is_noop: bool) -> Result<(), CmdError> {
    let mut last_conflict = None;
    for attempt in 0..3 {
        let (document, expected_generation) = super::registry::fetch_versioned_document().await?;
        let mut control = release_control::control(&document)?
            .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
        let policy = control.products.get(&args.product).ok_or_else(|| {
            CmdError::click(format!("unknown release product {:?}", args.product))
        })?;
        let mut artifacts = BTreeMap::new();
        let mut revisions = BTreeSet::new();
        for platform in platforms(policy) {
            let artifact =
                verified_artifact(&args.product, &args.version, &platform, &control).await?;
            revisions.insert(artifact.source_revision.clone());
            artifacts.insert(platform, artifact);
        }
        if revisions.len() != 1 {
            return Err(CmdError::click(
                "release platforms were not built from one source revision",
            ));
        }
        let channel = ReleaseChannel::from(args.channel);
        if exact_is_noop {
            if let Some(desired) = policy.desired.as_ref().filter(|desired| {
                desired.version == args.version
                    && desired.channel == channel
                    && desired.artifacts == artifacts
            }) {
                let report = json!({
                    "product": args.product,
                    "version": args.version,
                    "channel": channel,
                    "rollout_generation": desired.rollout_generation,
                    "registry_generation": control.generation,
                    "store_generation": expected_generation,

                });
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!(
                        "already promoted {} {} channel={:?} rollout-generation={} registry-generation={}",
                        args.product,
                        args.version,
                        channel,
                        desired.rollout_generation,
                        control.generation
                    );
                }
                return Ok(());
            }
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
            channel,
            rollout_generation,
            promoted_at: Utc::now().to_rfc3339(),
            artifacts,
        });
        control.generation = control.generation.saturating_add(1);
        let mut updated = document;
        updated[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&control)?;
        let stored_generation =
            match super::registry::push_document_if(&updated, &expected_generation).await {
                Ok(generation) => generation,
                Err(error)
                    if error
                        .to_string()
                        .contains("storage version changed for registry.json") =>
                {
                    last_conflict = Some(error);
                    if attempt < 2 {
                        tokio::time::sleep(std::time::Duration::from_millis(100 * (attempt + 1)))
                            .await;
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };
        let report = json!({
            "product": args.product,
            "version": args.version,
            "channel": channel,
            "rollout_generation": rollout_generation,
            "registry_generation": control.generation,
            "store_generation": stored_generation,
        });
        if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "promoted {} {} channel={:?} rollout-generation={} registry-generation={}",
                args.product, args.version, channel, rollout_generation, control.generation
            );
        }
        return Ok(());
    }
    Err(last_conflict
        .unwrap_or_else(|| CmdError::click("release promotion exhausted registry retries")))
}

pub(crate) async fn promote_for_submit(
    product: &str,
    version: &str,
    channel: crate::release_pipeline::PipelineChannel,
) -> Result<(), CmdError> {
    promote(
        &ReleasePromoteArgs {
            product: product.to_string(),
            version: version.to_string(),
            channel: match channel {
                crate::release_pipeline::PipelineChannel::Candidate => ChannelArg::Candidate,
                crate::release_pipeline::PipelineChannel::Stable => ChannelArg::Stable,
            },
            json: false,
        },
        true,
    )
    .await
}

async fn agent(args: &ReleaseAgentArgs) -> Result<(), CmdError> {
    let product = args.product.as_deref();
    let states = if args.once {
        crate::release_agent::reconcile_once(&args.target, product)
            .await
            .map_err(CmdError::click)?
    } else {
        return crate::release_agent::agent(&args.target, product, false, args.interval_seconds)
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

/// The binary one release policy installs on a host, as the concrete file the
/// host's software report names.
///
/// The rollout's own artefact lives under the product install root, so it is in
/// no `managed_versions` entry and in no `$HOME/.stado/bin` listing. A status
/// command that did not resolve this path could compare the desired version
/// against nothing at all — which is what `observed=unreported` was.
fn product_binary(policy: &ProductReleasePolicy) -> crate::host_software::ProductBinary {
    let path = format!(
        "{}/{}",
        policy.install_root.trim_end_matches('/'),
        policy.binary.trim_start_matches('/')
    );
    crate::host_software::ProductBinary {
        name: path
            .rsplit('/')
            .next()
            .unwrap_or(policy.binary.as_str())
            .to_string(),
        path,
        desired: policy
            .desired
            .as_ref()
            .map(|desired| desired.version.clone()),
    }
}

/// `stado release status [--product NAME] [--json]` — desired against observed,
/// and now against what the host actually runs.
///
/// Two things were true of this command until 2026-08-18 and both were wrong.
/// It printed `brama target=control-host desired=0.2.27 observed=unreported`
/// and exited **zero**, so a host that had never once said what it runs read as
/// a host with nothing to answer for. And `observed` came only from the release
/// agent's own state file, so software installed outside the release channel —
/// skarbiec 0.2.1 on one machine and 0.2.3 on another, neither in any published
/// release — was invisible here even while it was the thing breaking the fleet.
///
/// The third column closes both. It is the host's own software report
/// ([`crate::host_software`]), read out of the observation store rather than
/// gathered here: a status read must not cost one ssh connection per target, and
/// the age of the report is itself the finding when nobody has refreshed it.
/// Missing, stale, `unmanaged` and disagreeing are all failures, and the command
/// exits non-zero on any of them with one sentence per row naming the host and
/// the exact disagreement.
async fn status(args: &ReleaseStatusArgs) -> Result<(), CmdError> {
    let document = super::registry::fetch_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // One read of the observation file for the whole rendering, for the reason
    // `observations::describe_in` exists: a column whose cost scales with the
    // size of the fleet is a column somebody eventually deletes.
    let records = crate::observations::load();
    let mut reports = Vec::new();
    let mut failures = usize::default();
    for (product, policy) in &control.products {
        if args
            .product
            .as_deref()
            .is_some_and(|selected| selected != product)
        {
            continue;
        }
        for target in policy.targets.keys() {
            let uri = crate::release_agent::release_status_uri(product, target);
            let observed: Value = match crate::cli::storage::fetch_object(&uri).await {
                Ok(bytes) => serde_json::from_slice(&bytes)?,
                Err(_) => Value::Null,
            };
            let software = crate::host_software::load_in(&records, target);
            let declared = registry
                .targets
                .iter()
                .find(|entry| &entry.name == target)
                .map(|entry| entry.managed_versions.clone())
                .unwrap_or_default();
            let finding =
                crate::host_software::judge(&software, &declared, Some(&product_binary(policy)));
            if finding.failed {
                failures = failures.saturating_add(1);
            }
            let mut block = software.json();
            finding.merge_into(&mut block);
            reports.push(json!({
                "product": product,
                "target": target,
                "desired": policy.desired,
                "previous": policy.previous,
                "observed": observed,
                "software": block,
            }));
        }
    }
    let runs = super::release_submit::recent_runs(args.product.as_deref(), 10).await?;
    if reports.is_empty() && runs.is_empty() {
        return Err(CmdError::click("no matching release product"));
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"targets": reports, "runs": runs}))?
        );
    } else {
        for report in &reports {
            let software = &report["software"];
            println!(
                "{} target={} desired={} observed={} software={} ({} of {} unmanaged, reported {})",
                report["product"].as_str().unwrap_or_default(),
                report["target"].as_str().unwrap_or_default(),
                report["desired"]["version"].as_str().unwrap_or("-"),
                report["observed"]["phase"].as_str().unwrap_or("unreported"),
                software["verdict"].as_str().unwrap_or("failed"),
                software["unmanaged"].as_u64().unwrap_or_default(),
                software["reported"].as_u64().unwrap_or_default(),
                software["observed"].as_str().unwrap_or("never"),
            );
            // One sentence per row, on stdout beside the row it is about: an
            // operator reading a gate should not have to interleave two streams
            // to learn which target the accusation belongs to.
            for sentence in software["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                println!("  ! {sentence}");
            }
        }
        if !runs.is_empty() {
            println!("--- pipeline runs (newest first):");
        }
        for run in runs {
            println!(
                "run {} {} {} {} {} {}",
                &run["run_id"].as_str().unwrap_or("-")
                    [..8.min(run["run_id"].as_str().unwrap_or("-").len())],
                run["product"].as_str().unwrap_or("-"),
                run["version"].as_str().unwrap_or("-"),
                run["channel"].as_str().unwrap_or("-"),
                run["state"].as_str().unwrap_or("-"),
                run["updated_at"].as_str().unwrap_or("-"),
            );
            // Each platform on its own line: the run-level state alone reads
            // as a promise, while "linux-amd64 submitted job=4ffae52f
            // [running]" is a fact an operator can go and watch.
            for (platform, record) in run["platforms"].as_object().into_iter().flatten() {
                let mut line = format!(
                    "  {platform} {} job={}",
                    record["state"].as_str().unwrap_or("-"),
                    &record["job_id"].as_str().unwrap_or("-")
                        [..8.min(record["job_id"].as_str().unwrap_or("-").len())],
                );
                if let Some(job_state) = record["job_state"].as_str() {
                    line.push_str(&format!(" [{job_state}]"));
                }
                // An estimate and labelled as one: crates compiled so far
                // against this platform's previous run, because cargo
                // publishes no total of its own.
                if let Some(compiled) = record["compile_progress"]["compiled"].as_u64() {
                    match record["compile_progress"]["percent"].as_u64() {
                        Some(percent) => line.push_str(&format!(
                            " compiled {compiled} crates (~{percent}% of the previous run)"
                        )),
                        None => line.push_str(&format!(" compiled {compiled} crates")),
                    }
                }
                println!("{line}");
                if let Some(failure) = record["failure"].as_str() {
                    println!("    failure: {}", failure.lines().next().unwrap_or(failure));
                }
            }
            if let Some(failure) = run["failure"].as_str() {
                // One line of evidence, not the whole log: the first line
                // names the failing step and host; `submit --json` carries
                // the rest.
                println!("  failure: {}", failure.lines().next().unwrap_or(failure));
            }
        }
    }
    if failures == usize::default() {
        return Ok(());
    }
    // Silence is the failure. Every sentence is already printed beside its row,
    // so this exits without a second message: what a gate owes an operator is a
    // non-zero code and the reason, not the reason twice.
    eprintln!(
        "{failures} of {} rollout target(s) cannot be shown to be running what the fleet declares \
         for them; each is named above",
        reports.len()
    );
    Err(CmdError::silent(super::CLICK_ERROR_CODE))
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

/// Verify the delivered archive against the delivery contract's digest,
/// extract one member, and install it under `$HOME/.stado/bin` by rename —
/// Linux refuses to write into a running executable (ETXTBSY) but allows
/// replacing the name, and a dated backup is kept beside it.
async fn install_local(args: &ReleaseInstallLocalArgs) -> Result<(), CmdError> {
    use sha2::Digest as _;
    let archive = std::env::var("WISENT_RELEASE_ARCHIVE")
        .map_err(|_| CmdError::click("WISENT_RELEASE_ARCHIVE is not set; this command is the delivery contract's local endpoint"))?;
    let expected = std::env::var("WISENT_RELEASE_SHA256")
        .map_err(|_| CmdError::click("WISENT_RELEASE_SHA256 is not set; this command is the delivery contract's local endpoint"))?;
    let bytes = std::fs::read(&archive).map_err(|error| {
        CmdError::click(format!("cannot read delivered archive {archive}: {error}"))
    })?;
    let actual = hex::encode(sha2::Sha256::digest(&bytes));
    if actual != expected {
        return Err(CmdError::click(format!(
            "delivered archive digest mismatch: expected {expected}, got {actual}"
        )));
    }
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    let mut bundle = tar::Archive::new(decoder);
    let member = args.member.trim_start_matches('/');
    let mut payload: Option<Vec<u8>> = None;
    for entry in bundle
        .entries()
        .map_err(|error| CmdError::click(format!("unreadable release archive: {error}")))?
    {
        let mut entry =
            entry.map_err(|error| CmdError::click(format!("unreadable archive entry: {error}")))?;
        let path = entry
            .path()
            .map_err(|error| CmdError::click(format!("unreadable archive path: {error}")))?
            .to_string_lossy()
            .into_owned();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        if path == member || path.ends_with(&format!("/{member}")) {
            let mut content = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut content)
                .map_err(|error| CmdError::click(format!("cannot extract {member}: {error}")))?;
            payload = Some(content);
            break;
        }
    }
    let Some(content) = payload else {
        return Err(CmdError::click(format!(
            "release archive carries no regular member {member}"
        )));
    };
    let name = if args.name.is_empty() {
        args.member
            .rsplit('/')
            .next()
            .unwrap_or(args.member.as_str())
            .to_string()
    } else {
        args.name.clone()
    };
    let home = crate::config_file::expand_tilde("~");
    let directory = home.join(".stado").join("bin");
    std::fs::create_dir_all(&directory).map_err(|error| {
        CmdError::click(format!("cannot prepare {}: {error}", directory.display()))
    })?;
    // The installed coordinate is the cheap, persistent handshake between the
    // delivery child and already-running queue agents. Agents launched from
    // this managed path finish their current slot, compare this file with their
    // compiled version, then let their declared supervisor recreate them.
    let release_version_stage =
        if name == "stado" && std::env::var("WISENT_PRODUCT").ok().as_deref() == Some("stado") {
            let version = std::env::var("WISENT_VERSION")
                .map_err(|_| CmdError::click("WISENT_VERSION is not set for the Stado delivery"))?;
            let version = version.trim();
            if version.is_empty() {
                return Err(CmdError::click(
                    "WISENT_VERSION is empty for the Stado delivery",
                ));
            }
            let path = directory.join("stado.release-version.release-incoming");
            std::fs::write(&path, format!("{version}\n")).map_err(|error| {
                CmdError::click(format!(
                    "cannot stage the installed Stado release coordinate: {error}"
                ))
            })?;
            Some(path)
        } else {
            None
        };
    let destination = directory.join(&name);
    if destination.exists() {
        let stamp = chrono::Utc::now().format("%Y%m%d");
        let backup = directory.join(format!("{name}.release-backup-{stamp}"));
        if !backup.exists() {
            std::fs::copy(&destination, &backup)
                .map_err(|error| CmdError::click(format!("cannot back up {name}: {error}")))?;
        }
    }
    let staged = directory.join(format!("{name}.release-incoming"));
    std::fs::write(&staged, &content)
        .map_err(|error| CmdError::click(format!("cannot stage {name}: {error}")))?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| CmdError::click(format!("cannot mark {name} executable: {error}")))?;
    }
    // Leave the receipt the fleet's provenance check reads, before the
    // install replaces the name.
    //
    // `cli::service_converge::attest_installed` decides provenance by byte
    // comparing the installed file against
    // `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, which
    // `deploy::host_release` writes when IT delivers. This command is the
    // other delivery endpoint and it staged nothing, so a binary it installed
    // read `unattested` forever after — even though the archive was verified
    // against the contract digest a hundred lines above.
    //
    // lukasz-macbook is the proof. Its `~/.stado/bin` carries this command's
    // own dated backups through 2026-09-02 and its `stado.release-version`
    // handshake, so deliveries plainly ran; `~/.stado/releases/stado` holds
    // 0.13.24 and older, nothing since. `stado service converge` therefore
    // reported the host's binary as bytes the fleet cannot attest, and the
    // remediation it printed — deliver a published version — was the thing
    // that had just happened.
    //
    // Never fatal: the archive is verified and the install is the point, so a
    // receipt that cannot be written is named and the delivery continues.
    match (
        std::env::var("WISENT_VERSION")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty()),
        crate::self_update::platform_triple_short(),
    ) {
        (Some(version), Ok(platform)) => {
            if let Err(error) =
                crate::self_update::stage_for_attestation(&name, &version, platform, &staged)
            {
                println!(
                    "release install-local: {name} {version} installed but its attestation copy \
                     could not be staged, so `stado service converge` will read it as \
                     unattested: {error}"
                );
            }
        }
        (None, _) => println!(
            "release install-local: WISENT_VERSION is unset, so no attestation copy was staged \
             and `stado service converge` will read {name} as unattested"
        ),
        (_, Err(error)) => println!(
            "release install-local: this platform has no release triple ({error}), so no \
             attestation copy was staged for {name}"
        ),
    }
    std::fs::rename(&staged, &destination)
        .map_err(|error| CmdError::click(format!("cannot install {name}: {error}")))?;
    if let Some(staged_version) = release_version_stage {
        let installed_version = directory.join("stado.release-version");
        std::fs::rename(&staged_version, &installed_version).map_err(|error| {
            CmdError::click(format!(
                "cannot activate the installed Stado release coordinate: {error}"
            ))
        })?;
    }
    // The handshake above is the queue agent's, and only the queue agent
    // implements it: `providers::local::agent` is the sole reader of
    // `stado.release-version`. Every other unit launched from this directory
    // — the disk-cleanup janitor, the resolver, the health beacon — keeps
    // executing the inode it started with, for as long as it lives, because
    // nothing tells launchd or systemd that the file underneath changed.
    //
    // That is how a delivery could succeed and change nothing. On 2026-09-01
    // the janitor on lukasz-macbook was still executing a 68,977,488-byte
    // image of this exact path while the file was 70,265,008 bytes, had
    // answered `invalid_or_unavailable_policy` 8,460 times out of 12,009
    // passes because the policy no longer validated against the code it was
    // compiled from, and the volume had reached 100% full with a janitor
    // running every minute the whole way down.
    //
    // In place, and never the agent: see `self_update::recycle_replaced_units`.
    let mut recycle_log = |message: &str| println!("{message}");
    crate::self_update::recycle_replaced_units(
        "release install-local",
        &directory,
        std::slice::from_ref(&name),
        &mut recycle_log,
    )
    .await;
    println!(
        "installed {} from the delivered release archive",
        destination.display()
    );
    Ok(())
}
/// Claim one coordinate from the command line and report what was found.
async fn claim_coordinate(args: &ReleaseClaimCoordinateArgs) -> Result<(), CmdError> {
    let outcome = claim_release_coordinate(
        &args.product,
        &args.version,
        &args.platform,
        &args.source_commit,
    )
    .await?;
    let base = release_control::release_base(&args.product, &args.version, &args.platform)
        .map_err(CmdError::click)?;
    let uri = format!("{base}/{}", release_control::RELEASE_REVISION_NAME);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "state": outcome.label(),
                "uri": uri,
                "product": args.product,
                "version": args.version,
                "platform": args.platform,
                "source_revision": args.source_commit,
            }))?
        );
    } else {
        println!(
            "{} {} {} {} at source revision {}",
            outcome.label(),
            args.product,
            args.version,
            args.platform,
            args.source_commit
        );
    }
    Ok(())
}

pub async fn dispatch(command: ReleaseCommands) -> Result<(), CmdError> {
    match command {
        ReleaseCommands::Keygen(args) => keygen(&args).await,
        ReleaseCommands::PolicyApply(args) => apply_policy(&args).await,
        ReleaseCommands::Submit(args) => crate::cli::release_submit::submit(&args).await,
        ReleaseCommands::Catalog(args) => crate::cli::release_catalog::dispatch(args).await,
        ReleaseCommands::Worker(args) => crate::cli::release_submit::worker(&args).await,
        ReleaseCommands::DeliveryWorker(args) => {
            crate::cli::release_submit::delivery_worker(&args).await
        }
        ReleaseCommands::Prepare(args) => prepare(&args).await,
        ReleaseCommands::Promote(args) => promote(&args, false).await,
        ReleaseCommands::Agent(args) => agent(&args).await,
        ReleaseCommands::Proxy(args) => crate::release_agent::proxy(&args.state, &args.bind)
            .await
            .map_err(CmdError::click),
        ReleaseCommands::Status(args) => status(&args).await,
        ReleaseCommands::Logs(args) => crate::cli::release_evidence::dispatch_logs(&args).await,
        ReleaseCommands::Doctor(args) => crate::cli::release_evidence::dispatch_doctor(&args).await,
        ReleaseCommands::Quarantine(sub) => crate::cli::release_quarantine::dispatch(sub).await,
        ReleaseCommands::Rollback(args) => rollback(&args).await,
        ReleaseCommands::InstallLocal(args) => install_local(&args).await,
        ReleaseCommands::ClaimCoordinate(args) => claim_coordinate(&args).await,
    }
}
