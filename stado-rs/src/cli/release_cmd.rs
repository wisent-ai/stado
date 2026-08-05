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

use super::CmdError;

#[derive(Subcommand)]
pub enum ReleaseCommands {
    /// Generate an Ed25519 release authority key pair.
    Keygen(ReleaseKeygenArgs),
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
    signing_key: PathBuf,
    #[arg(long)]
    key_id: String,
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

fn qualification(path: &Path) -> Result<ReleaseQualification, CmdError> {
    let value: ReleaseQualification = serde_json::from_slice(&std::fs::read(path)?)?;
    Ok(value)
}

async fn prepare(args: &ReleasePrepareArgs) -> Result<(), CmdError> {
    let (artifact_bytes, artifact_sha256) =
        release_control::sha256_file(&args.archive).map_err(CmdError::click)?;
    let private = std::fs::read(&args.signing_key)?;
    let public = release_control::signing_public_key(&private).map_err(CmdError::click)?;
    let manifest = ReleaseManifest {
        schema_version: 1,
        product: args.product.clone(),
        version: args.version.clone(),
        platform: args.platform.clone(),
        source_revision: args.source_revision.to_lowercase(),
        artifact_sha256,
        artifact_bytes,
        binary: args.binary.clone(),
        launcher: args.launcher.clone(),
        config_schema: args.config_schema,
        state_schema: args.state_schema,
        minimum_stado_version: args.minimum_stado_version.clone(),
        rollback_compatible_with: args.rollback_compatible_with.clone(),
        qualification: qualification(&args.qualification)?,
        key_id: args.key_id.clone(),
        built_at: Utc::now().to_rfc3339(),
        builder: args.builder.clone(),
    };
    release_control::validate_manifest(&manifest).map_err(CmdError::click)?;
    let manifest_bytes = release_control::canonical_manifest(&manifest).map_err(CmdError::click)?;
    let signature = release_control::sign_manifest(&private, &manifest).map_err(CmdError::click)?;
    let base = release_control::release_base(&args.product, &args.version, &args.platform)
        .map_err(CmdError::click)?;
    let archive_uri = format!("{base}/{}", release_control::RELEASE_ARCHIVE_NAME);
    let signature_uri = format!("{base}/{}", release_control::RELEASE_SIGNATURE_NAME);
    let manifest_uri = format!("{base}/{}", release_control::RELEASE_MANIFEST_NAME);
    let archive = std::fs::read(&args.archive)?;
    put_immutable(&archive_uri, &archive, "application/gzip").await?;
    put_immutable(
        &signature_uri,
        format!("{signature}\n").as_bytes(),
        "text/plain",
    )
    .await?;
    // Manifest is the commit marker and is deliberately published last.
    put_immutable(&manifest_uri, &manifest_bytes, "application/json").await?;
    let report = json!({
        "product": args.product,
        "version": args.version,
        "platform": args.platform,
        "source_revision": args.source_revision,
        "artifact_sha256": manifest.artifact_sha256,
        "manifest_sha256": release_control::sha256_bytes(&manifest_bytes),
        "key_id": args.key_id,
        "public_key": BASE64.encode(public),
        "qualification": manifest.qualification.status,
        "archive_uri": archive_uri,
        "signature_uri": signature_uri,
        "manifest_uri": manifest_uri,
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

async fn verified_artifact(
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
