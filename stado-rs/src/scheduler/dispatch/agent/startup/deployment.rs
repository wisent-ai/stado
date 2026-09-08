//! The non-secret deployment settings every agent template substitutes.

use std::collections::BTreeMap;

use crate::config;

/// Non-secret `${KEY}` substitutions the agent templates may reference,
/// read from the process config. Deliberately NOT merged into the
/// coordinator's secrets map: these are deployment settings, not
/// credentials, and keeping the key set here — next to the templates that
/// consume it — gives a new placeholder exactly one place to be
/// registered instead of one per producer.
///
/// Every provider receives the complete primary/backup storage binding, scoped
/// agent identity, canonical provider kind, and immutable binary/runtime
/// coordinates. Secrets stay in the coordinator map: Azure consumes its raw
/// grant through protected settings, while other remote agents receive only
/// the scoped workload grant projection their root startup script materializes.
pub fn deployment_substitutions(provider_name: &str) -> BTreeMap<String, String> {
    let field = |adapter, key| {
        crate::capabilities::config_field(crate::capabilities::RuntimeFacet::Storage, adapter, key)
            .expect("storage binding is missing from the capability catalog")
    };
    let gcs_bucket = field(crate::capabilities::StorageAdapter::Gcs.id(), "bucket");
    let azure_account = field(
        crate::capabilities::StorageAdapter::AzureBlob.id(),
        "account",
    );
    let azure_container = field(
        crate::capabilities::StorageAdapter::AzureBlob.id(),
        "container",
    );
    let s3_bucket = field(crate::capabilities::StorageAdapter::S3.id(), "bucket");
    let s3_region = field(crate::capabilities::StorageAdapter::S3.id(), "region");
    let local_path = field(crate::capabilities::StorageAdapter::Local.id(), "path");
    let stado_url = field(crate::capabilities::StorageAdapter::StadoObject.id(), "url");
    let stado_token_file = field(
        crate::capabilities::StorageAdapter::StadoObject.id(),
        "token-file",
    );
    let stado_namespace = field(
        crate::capabilities::StorageAdapter::StadoObject.id(),
        "namespace",
    );
    let provider_kind =
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::Execution, provider_name)
            .map(|variant| variant.id)
            .unwrap_or(provider_name);
    BTreeMap::from([
        ("PROVIDER_KIND".to_string(), provider_kind.to_string()),
        (
            crate::capabilities::STORAGE_BACKEND_CONFIG.env.to_string(),
            config::wc_storage_backend().to_string(),
        ),
        (gcs_bucket.env.to_string(), config::bucket().to_string()),
        (
            azure_account.env.to_string(),
            config::wc_azure_storage_account().to_string(),
        ),
        (
            azure_container.env.to_string(),
            config::wc_azure_container().to_string(),
        ),
        (
            s3_bucket.env.to_string(),
            config::wc_s3_bucket().to_string(),
        ),
        (
            s3_region.env.to_string(),
            config::wc_s3_region().to_string(),
        ),
        (
            local_path.env.to_string(),
            config::wc_local_storage_path().to_string(),
        ),
        (
            stado_url.env.to_string(),
            config::wc_stado_storage_url().to_string(),
        ),
        (
            stado_token_file.env.to_string(),
            config::wc_stado_storage_token_file().to_string(),
        ),
        (
            stado_namespace.env.to_string(),
            config::wc_stado_storage_namespace().to_string(),
        ),
        (
            crate::capabilities::STORAGE_BACKEND_CONFIG
                .backup_env
                .expect("backup storage backend environment binding is missing")
                .to_string(),
            config::wc_backup_storage_backend().to_string(),
        ),
        (
            gcs_bucket
                .backup_env
                .expect("backup bucket environment binding is missing")
                .to_string(),
            config::wc_backup_bucket().to_string(),
        ),
        (
            azure_account
                .backup_env
                .expect("backup Azure account environment binding is missing")
                .to_string(),
            config::wc_backup_azure_storage_account().to_string(),
        ),
        (
            azure_container
                .backup_env
                .expect("backup Azure container environment binding is missing")
                .to_string(),
            config::wc_backup_azure_container().to_string(),
        ),
        (
            s3_region
                .backup_env
                .expect("backup S3 region environment binding is missing")
                .to_string(),
            config::wc_backup_s3_region().to_string(),
        ),
        (
            local_path
                .backup_env
                .expect("backup local path environment binding is missing")
                .to_string(),
            config::wc_backup_local_storage_path().to_string(),
        ),
        (
            "WC_AGENT_SKARBIEC_URL".to_string(),
            config::agent_skarbiec_url().to_string(),
        ),
        (
            "WC_AGENT_SKARBIEC_CONSUMER".to_string(),
            config::agent_skarbiec_consumer().to_string(),
        ),
        (
            "WC_AGENT_SKARBIEC_ITEMS".to_string(),
            config::agent_skarbiec_items().join(","),
        ),
        (
            "WC_AGENT_SKARBIEC_SECRET_FIELDS".to_string(),
            config::agent_skarbiec_secret_fields().join(","),
        ),
        ("STADO_API_URL".to_string(), config::stado_api_url()),
        (
            "STADO_RELEASE_VERSION".to_string(),
            config::stado_release_version(),
        ),
        (
            "STADO_RELEASE_PLATFORM".to_string(),
            config::stado_release_platform(),
        ),
        (
            "STADO_AGENT_RUNTIME_BUNDLE_URI".to_string(),
            config::stado_agent_runtime_bundle_uri(),
        ),
        (
            "STADO_AGENT_RUNTIME_BUNDLE_SHA256".to_string(),
            config::stado_agent_runtime_bundle_sha256(),
        ),
        ("AWS_REGION".to_string(), config::aws_region().to_string()),
    ])
}
