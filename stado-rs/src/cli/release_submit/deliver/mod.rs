//! Delivery: the immutable request one delivery job carries, the receipt it
//! leaves, and the redelivery of a single named delivery.

pub(in crate::cli::release_submit) mod deliveries;
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
        crate::constants::RELEASE_DELIVERY_JOB_COMMAND
    } else {
        crate::constants::PRODUCT_RELEASE_DELIVERY_JOB_COMMAND
    }
}
