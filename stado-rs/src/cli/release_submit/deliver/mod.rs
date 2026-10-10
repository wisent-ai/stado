//! Delivery: the immutable request one delivery job carries, the receipt it
//! leaves, and the redelivery of a single named delivery.

pub(in crate::cli::release_submit) mod deliveries;
pub(in crate::cli::release_submit) mod placement;
mod queue;
pub(in crate::cli::release_submit) mod redelivery;
pub(in crate::cli::release_submit) mod worker;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::release_pipeline::StepStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryRequest {
    schema_version: u32,
    run_id: String,
    name: String,
    product: String,
    version: String,
    platform: String,
    argv: Vec<String>,
    required: bool,
    secret_env: BTreeMap<String, String>,
    source_path: String,
    source_uri: String,
    source_sha256: String,
    archive_path: String,
    archive_uri: String,
    archive_sha256: String,
    manifest_uri: String,
    manifest_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryReceipt {
    schema_version: u32,
    run_id: String,
    job_id: String,
    name: String,
    product: String,
    version: String,
    platform: String,
    argv: Vec<String>,
    required: bool,
    secret_env: BTreeMap<String, String>,
    archive_uri: String,
    archive_sha256: String,
    manifest_uri: String,
    manifest_sha256: String,
    status: StepStatus,
    exit_code: Option<i32>,
    completed_at: String,
}

fn delivery_job_command(product: &str) -> &'static str {
    if product == "stado" {
        crate::primitives::constants::RELEASE_DELIVERY_JOB_COMMAND
    } else {
        crate::primitives::constants::PRODUCT_RELEASE_DELIVERY_JOB_COMMAND
    }
}

/// The program a Stado delivery may name first, which the worker runs as
/// its own current executable rather than looking it up.
const OWN_PROGRAM: &str = "stado";

/// Whether the worker runs `request`'s first argument as its own current
/// executable: a Stado delivery whose argv starts with the Stado program. The
/// receipt then records that executable's path in place of the name.
fn runs_own_executable(request: &DeliveryRequest) -> bool {
    request.product == OWN_PROGRAM
        && request
            .argv
            .first()
            .is_some_and(|program| program == OWN_PROGRAM)
}

/// The receipt job `job_id` left for `request`, checked against it: present,
/// decodable, written for this run, job, delivery and published artifact,
/// with the declared arguments the worker ran and a passed step. Answers the
/// receipt's SHA-256, or every way the receipt fails to prove the delivery.
///
/// A completed queue job used to pass its delivery on the queue's word: an
/// absent, malformed or unrelated receipt still passed the leg and admitted
/// the deliveries that come after it.
fn verified_receipt(
    bytes: Option<&[u8]>,
    request: &DeliveryRequest,
    job_id: &str,
) -> Result<String, String> {
    let bytes = bytes.ok_or_else(|| format!("job {job_id} left no delivery-receipt.json"))?;
    let receipt: DeliveryReceipt = serde_json::from_slice(bytes).map_err(|error| {
        format!("the delivery-receipt.json of job {job_id} does not decode: {error}")
    })?;
    let mut found = Vec::new();
    for (field, written, declared) in [
        ("run", receipt.run_id.as_str(), request.run_id.as_str()),
        ("job", receipt.job_id.as_str(), job_id),
        ("delivery", receipt.name.as_str(), request.name.as_str()),
        (
            "product",
            receipt.product.as_str(),
            request.product.as_str(),
        ),
        (
            "version",
            receipt.version.as_str(),
            request.version.as_str(),
        ),
        (
            "platform",
            receipt.platform.as_str(),
            request.platform.as_str(),
        ),
        (
            "archive uri",
            receipt.archive_uri.as_str(),
            request.archive_uri.as_str(),
        ),
        (
            "archive sha256",
            receipt.archive_sha256.as_str(),
            request.archive_sha256.as_str(),
        ),
        (
            "manifest uri",
            receipt.manifest_uri.as_str(),
            request.manifest_uri.as_str(),
        ),
        (
            "manifest sha256",
            receipt.manifest_sha256.as_str(),
            request.manifest_sha256.as_str(),
        ),
    ] {
        if written != declared {
            found.push(format!(
                "its {field} is {written}, the request declares {declared}"
            ));
        }
    }
    if receipt.schema_version != request.schema_version {
        found.push(format!(
            "its schema version is {}, the request's is {}",
            receipt.schema_version, request.schema_version
        ));
    }
    if receipt.required != request.required {
        found.push(format!(
            "it says required {}, the request {}",
            receipt.required, request.required
        ));
    }
    if receipt.secret_env != request.secret_env {
        found.push(format!(
            "its secret references are {:?}, the request declares {:?}",
            receipt.secret_env, request.secret_env
        ));
    }
    let ran_declared = if runs_own_executable(request) {
        match (receipt.argv.split_first(), request.argv.split_first()) {
            (Some((_, written)), Some((_, declared))) => written == declared,
            _ => false,
        }
    } else {
        receipt.argv == request.argv
    };
    if !ran_declared {
        found.push(format!(
            "it ran {:?}, the request declares {:?}",
            receipt.argv, request.argv
        ));
    }
    if receipt.status != StepStatus::Passed {
        found.push(format!(
            "its step {:?} with exit code {:?}",
            receipt.status, receipt.exit_code
        ));
    }
    if found.is_empty() {
        return Ok(crate::release_control::sha256_bytes(bytes));
    }
    Err(format!(
        "the delivery-receipt.json of job {job_id} does not prove delivery {}: {}",
        request.name,
        found.join("; ")
    ))
}
