//! Storage variants: the stores a deployment's queue and objects can live in.
//! Every named S3 API provider (`queue::s3::vendor`) is a variant of its own,
//! selected by its provider id, so `storage.backend` names the provider and
//! the configuration asks only for what that provider's endpoint is built
//! from.

use crate::capabilities::catalog::ProviderId;
use crate::capabilities::config::compatible::{
    S3_COMPATIBLE_CONFIG, S3_VENDOR_ACCOUNT_CONFIG, S3_VENDOR_ACCOUNT_REGION_CONFIG,
    S3_VENDOR_FIXED_CONFIG, S3_VENDOR_REGION_CONFIG, S3_VENDOR_SELF_HOSTED_CONFIG,
};
use crate::capabilities::config::variants::{
    AZURE_STORAGE_CONFIG, GCS_CONFIG, LOCAL_STORAGE_CONFIG, S3_CONFIG, STADO_OBJECT_STORAGE_CONFIG,
};
use crate::capabilities::config::ConfigField;
use crate::capabilities::registry::CapabilityVariant;
use crate::capabilities::runtime::{RuntimeAdapter, StorageAdapter};
use crate::queue::s3::vendor::{Declares, StorageVendor};

/// The configuration one named provider's backend reads.
const fn vendor_config(declares: Declares) -> &'static [ConfigField] {
    match declares {
        Declares::Account => S3_VENDOR_ACCOUNT_CONFIG,
        Declares::Region => S3_VENDOR_REGION_CONFIG,
        Declares::AccountAndRegion => S3_VENDOR_ACCOUNT_REGION_CONFIG,
        Declares::Nothing => S3_VENDOR_FIXED_CONFIG,
        Declares::EndpointAndRegion => S3_VENDOR_SELF_HOSTED_CONFIG,
    }
}

/// One named S3 API provider as a storage variant: its id and provider are
/// the provider's, its summary the provider's name for its store.
const fn vendor(vendor: StorageVendor) -> CapabilityVariant {
    let profile = vendor.profile();
    CapabilityVariant {
        id: vendor.id(),
        aliases: profile.provider.aliases(),
        provider: Some(profile.provider),
        implementation: "queue::s3::S3Backend::compatible + queue::s3::vendor",
        summary: profile.product,
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Storage(StorageAdapter::S3Vendor(vendor)),
        config: vendor_config(profile.declares),
    }
}

pub(in crate::capabilities::registry) const STORAGE: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: StorageAdapter::Gcs.id(),
        aliases: &[],
        provider: Some(ProviderId::Gcp),
        implementation: "queue::gcs::GcsBackend",
        summary: "Shared Google Cloud Storage queue and object store.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Storage(StorageAdapter::Gcs),
        config: GCS_CONFIG,
    },
    CapabilityVariant {
        id: StorageAdapter::AzureBlob.id(),
        aliases: &[],
        provider: Some(ProviderId::Azure),
        implementation: "queue::azure_blob::AzureBlobBackend",
        summary: "Shared Azure Blob queue and object store.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Storage(StorageAdapter::AzureBlob),
        config: AZURE_STORAGE_CONFIG,
    },
    CapabilityVariant {
        id: StorageAdapter::S3.id(),
        aliases: &[],
        provider: Some(ProviderId::Aws),
        implementation: "queue::s3::S3Backend",
        summary: "Shared Amazon S3 queue and object store.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Storage(StorageAdapter::S3),
        config: S3_CONFIG,
    },
    CapabilityVariant {
        id: StorageAdapter::S3Compatible.id(),
        aliases: &["s3-api"],
        provider: Some(ProviderId::S3Compatible),
        implementation: "queue::s3::S3Backend::compatible",
        summary: "Shared queue and object store on any S3 API endpoint named by URL that no \
                  named provider covers.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Storage(StorageAdapter::S3Compatible),
        config: S3_COMPATIBLE_CONFIG,
    },
    vendor(StorageVendor::CloudflareR2),
    vendor(StorageVendor::BackblazeB2),
    vendor(StorageVendor::Wasabi),
    vendor(StorageVendor::DigitalOceanSpaces),
    vendor(StorageVendor::Hetzner),
    vendor(StorageVendor::Scaleway),
    vendor(StorageVendor::Ovhcloud),
    vendor(StorageVendor::Akamai),
    vendor(StorageVendor::Vultr),
    vendor(StorageVendor::Oracle),
    vendor(StorageVendor::Ibm),
    vendor(StorageVendor::Alibaba),
    vendor(StorageVendor::Tencent),
    vendor(StorageVendor::Huawei),
    vendor(StorageVendor::Exoscale),
    vendor(StorageVendor::Ionos),
    vendor(StorageVendor::Contabo),
    vendor(StorageVendor::UpCloud),
    vendor(StorageVendor::Storj),
    vendor(StorageVendor::Filebase),
    vendor(StorageVendor::Tigris),
    vendor(StorageVendor::Yandex),
    vendor(StorageVendor::IdriveE2),
    vendor(StorageVendor::Minio),
    vendor(StorageVendor::Garage),
    vendor(StorageVendor::Seaweedfs),
    vendor(StorageVendor::Ceph),
    CapabilityVariant {
        id: StorageAdapter::Local.id(),
        aliases: &[],
        provider: Some(ProviderId::Local),
        implementation: "queue::local_file::LocalBackend",
        summary: "Device-local filesystem store for one-host operation.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Storage(StorageAdapter::Local),
        config: LOCAL_STORAGE_CONFIG,
    },
    CapabilityVariant {
        id: StorageAdapter::StadoObject.id(),
        aliases: &["stado-object", "self-hosted"],
        provider: Some(ProviderId::Stado),
        implementation: "queue::stado_object::StadoObjectBackend",
        summary: "Self-hosted: a Stado object API on your own machine and disk, reached by every \
                  host of the fleet; it also serves the S3 API at /s3.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Storage(StorageAdapter::StadoObject),
        config: STADO_OBJECT_STORAGE_CONFIG,
    },
];
