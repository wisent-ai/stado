//! Every refusal that can be decided before a store is touched.
//!
//! The command's whole safety argument is that an impossible request never
//! reaches step one. So the endpoint locators, the writer/offline assertion,
//! the provider allowlist, and the billing-window confirmation are all settled
//! here, against nothing but the parsed arguments.

use std::collections::BTreeSet;

use crate::cli::recovery::request::RecoveryMigrateArgs;
use crate::cli::CmdError;
use crate::queue::copy::Endpoint;

pub(in crate::cli::recovery) fn validate_args(
    args: &RecoveryMigrateArgs,
    source: &Endpoint,
    destination: &Endpoint,
) -> Result<(), CmdError> {
    if source.describe() == destination.describe() {
        return Err(CmdError::usage(format!(
            "source and destination are the same store ({})",
            source.describe()
        )));
    }
    validate_endpoint(source, "source")?;
    validate_endpoint(destination, "destination")?;
    if args.source_offline && !args.writers.is_empty() {
        return Err(CmdError::usage("--source-offline conflicts with --writer; either list every writer or assert that none can run"));
    }
    if !args.source_offline && args.writers.is_empty() {
        return Err(CmdError::usage("list every source writer with --writer HOST:SERVICE, or pass --source-offline after independently disabling every writer"));
    }
    let mut providers = BTreeSet::new();
    for provider in &args.enable_providers {
        let variant = crate::capabilities::configurable_variant(
            crate::capabilities::RuntimeFacet::Compute,
            provider,
        )
        .ok_or_else(|| CmdError::usage(format!("unknown compute provider {provider:?}")))?;
        if !providers.insert(variant.id) {
            return Err(CmdError::usage(format!(
                "--enable-provider {} was repeated",
                variant.id
            )));
        }
    }
    if args.resume && args.activate.is_empty() {
        return Err(CmdError::usage(
            "--resume requires at least one explicitly selected --activate HOST:SERVICE",
        ));
    }
    if args.manage_gcp_billing {
        let gcs_source = source.adapter() == Some(crate::capabilities::StorageAdapter::Gcs);
        if !gcs_source {
            return Err(CmdError::usage(
                "--manage-gcp-billing is valid only when --from gcs",
            ));
        }
        let project = required(args.gcp_project.as_deref(), "--gcp-project")?;
        validate_gcp_project(project)?;
        let account = required(args.gcp_billing_account.as_deref(), "--gcp-billing-account")?;
        validate_billing_account(account)?;
        let confirmation = required(
            args.confirm_billing_window.as_deref(),
            "--confirm-billing-window",
        )?;
        if confirmation != project {
            return Err(CmdError::usage(format!(
                "--confirm-billing-window must exactly equal {project:?}"
            )));
        }
    } else if args.gcp_project.is_some()
        || args.gcp_billing_account.is_some()
        || args.confirm_billing_window.is_some()
    {
        return Err(CmdError::usage(
            "GCP billing flags require --manage-gcp-billing",
        ));
    }
    Ok(())
}

fn validate_endpoint(endpoint: &Endpoint, label: &str) -> Result<(), CmdError> {
    let missing = match endpoint.adapter() {
        Some(
            crate::capabilities::StorageAdapter::Gcs | crate::capabilities::StorageAdapter::S3,
        ) if endpoint.bucket.is_empty() => Some("bucket"),
        Some(crate::capabilities::StorageAdapter::AzureBlob) if endpoint.account.is_empty() => {
            Some("storage account")
        }
        Some(crate::capabilities::StorageAdapter::AzureBlob) if endpoint.container.is_empty() => {
            Some("container")
        }
        Some(crate::capabilities::StorageAdapter::Local) if endpoint.path.is_empty() => {
            Some("path")
        }
        _ => None,
    };
    if let Some(locator) = missing {
        return Err(CmdError::usage(format!(
            "{label} {} endpoint needs a {locator}",
            endpoint.kind
        )));
    }
    Ok(())
}

fn validate_gcp_project(project: &str) -> Result<(), CmdError> {
    let valid = !project.is_empty()
        && project
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(CmdError::usage(format!(
            "invalid GCP project id {project:?}"
        )))
    }
}

fn validate_billing_account(account: &str) -> Result<(), CmdError> {
    let Some(id) = account.strip_prefix("billingAccounts/") else {
        return Err(CmdError::usage(
            "--gcp-billing-account must be a full billingAccounts/... name",
        ));
    };
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(CmdError::usage(format!(
            "invalid billing account name {account:?}"
        )));
    }
    Ok(())
}

pub(in crate::cli::recovery) fn required<'a>(
    value: Option<&'a str>,
    flag: &str,
) -> Result<&'a str, CmdError> {
    value.ok_or_else(|| CmdError::usage(format!("{flag} is required")))
}
