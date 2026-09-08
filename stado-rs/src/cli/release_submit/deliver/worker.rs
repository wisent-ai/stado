//! `stado release delivery-worker` — the target-side half of one delivery,
//! fenced against every coordinate but its own run's current one.

use std::collections::BTreeMap;

use chrono::Utc;

use crate::cli::release_submit::builds::worker::steps::execute;
use crate::cli::release_submit::deliver::{DeliveryReceipt, DeliveryRequest};
use crate::cli::release_submit::run::state::{latest_submitted_run, load};
use crate::cli::release_submit::DeliveryWorkerArgs;
use crate::cli::CmdError;
use crate::release_control;
use crate::release_pipeline::{PlatformRunState, ReleaseRunState, StepStatus};

async fn require_current_delivery(request: &DeliveryRequest) -> Result<(), CmdError> {
    let latest = latest_submitted_run(&request.product)
        .await?
        .ok_or_else(|| {
            CmdError::click(format!(
                "delivery names no submitted release run for {}",
                request.product
            ))
        })?;
    let latest_exact = latest.run_id == request.run_id
        && latest.version == request.version
        && latest.source_sha256 == request.source_sha256
        && latest.source_uri == request.source_uri;
    if !latest_exact {
        return Err(CmdError::click(format!(
            "delivery for {} {} source {} was superseded by release run {} source {}; refusing \
             the stale coordinate",
            request.product,
            request.version,
            request.source_sha256,
            latest.run_id,
            latest.source_sha256
        )));
    }
    let run = load(&request.run_id).await?.ok_or_else(|| {
        CmdError::click(format!(
            "delivery names missing release run {}",
            request.run_id
        ))
    })?;
    let platform = run.platforms.get(&request.platform);
    let exact = run.state == ReleaseRunState::Delivering
        && run.product == request.product
        && run.version == request.version
        && run.source_sha256 == request.source_sha256
        && run.source_uri == request.source_uri
        && platform.is_some_and(|platform| {
            platform.state == PlatformRunState::Published
                && platform.artifact_sha256.as_deref() == Some(request.archive_sha256.as_str())
                && platform.release_manifest_sha256.as_deref()
                    == Some(request.manifest_sha256.as_str())
        });
    if !exact {
        return Err(CmdError::click(format!(
            "delivery {} {} {} does not match its current published run; refusing the stale \
             coordinate",
            request.product, request.version, request.platform
        )));
    }
    Ok(())
}

pub async fn delivery_worker(args: &DeliveryWorkerArgs) -> Result<(), CmdError> {
    let request: DeliveryRequest = serde_json::from_slice(&std::fs::read(&args.request)?)?;
    require_current_delivery(&request).await?;
    let archive = std::fs::read(&request.archive_path)?;
    let source_archive = std::fs::read(&request.source_path)?;
    if request.schema_version != 1
        || release_control::sha256_bytes(&archive) != request.archive_sha256
        || release_control::sha256_bytes(&source_archive) != request.source_sha256
    {
        return Err(CmdError::click("delivery input identity mismatch"));
    }
    let source_root = std::env::current_dir()?.join("delivery-source");
    release_control::safe_extract_archive(&source_archive, &source_root)
        .map_err(CmdError::click)?;
    let output = std::env::current_dir()?.join("output");
    std::fs::create_dir_all(&output)?;
    let environment = BTreeMap::from([
        (
            "WISENT_SOURCE_DIR".into(),
            source_root.display().to_string(),
        ),
        (
            "WISENT_RELEASE_ARCHIVE".into(),
            std::fs::canonicalize(&request.archive_path)?
                .display()
                .to_string(),
        ),
        ("WISENT_RELEASE_URI".into(), request.archive_uri.clone()),
        (
            "WISENT_RELEASE_SHA256".into(),
            request.archive_sha256.clone(),
        ),
        (
            "WISENT_RELEASE_MANIFEST_URI".into(),
            request.manifest_uri.clone(),
        ),
        (
            "WISENT_RELEASE_MANIFEST_SHA256".into(),
            request.manifest_sha256.clone(),
        ),
        ("WISENT_PRODUCT".into(), request.product.clone()),
        ("WISENT_VERSION".into(), request.version.clone()),
        ("WISENT_PLATFORM".into(), request.platform.clone()),
        ("WISENT_OUTPUT_DIR".into(), output.display().to_string()),
    ]);
    let mut delivery_argv = request.argv.clone();
    if request.product == "stado" && delivery_argv.first().is_some_and(|value| value == "stado") {
        delivery_argv[0] = std::env::current_exe()?.display().to_string();
    }
    let step = execute(&request.name, &delivery_argv, &source_root, &environment)?;
    let receipt = DeliveryReceipt {
        schema_version: 1,
        run_id: request.run_id,
        job_id: std::env::var("WC_JOB_ID").unwrap_or_default(),
        name: request.name,
        product: request.product,
        version: request.version,
        platform: request.platform,
        argv: delivery_argv,
        required: request.required,
        secret_env: request.secret_env,
        archive_uri: request.archive_uri,
        archive_sha256: request.archive_sha256,
        manifest_uri: request.manifest_uri,
        manifest_sha256: request.manifest_sha256,
        status: step.status.clone(),
        exit_code: step.exit_code,
        completed_at: Utc::now().to_rfc3339(),
    };
    std::fs::write(
        output.join("delivery-receipt.json"),
        serde_json::to_vec(&receipt)?,
    )?;
    if step.status == StepStatus::Passed {
        Ok(())
    } else {
        Err(CmdError::click("release delivery failed"))
    }
}
