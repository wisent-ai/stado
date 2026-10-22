//! Who stores the fleet's objects, and who can be the source or destination
//! of a copy: every store the storage registry can construct, named.

use super::super::macros::provider_rows;
use super::super::providers::ProviderId;
use super::super::support::{CapabilitySupport, ProviderCapability};

pub(super) const OBJECT_STORAGE: &[ProviderCapability] = provider_rows![
    ProviderId::Stado => (Implemented, "queue::stado_object::StadoObjectBackend + dashboard object API + dashboard S3 endpoint + cli::storage::bucket", "Self-hosted: your own machine and disk serve the fleet's objects through the Stado object API and the S3 API at /s3; stado storage bucket creates, lists, rotates and deletes its buckets and keys"),
    ProviderId::Gcp => (Implemented, "queue::gcs::GcsBackend", "Preview Google Cloud Storage; not stable without release-scoped live acceptance"),
    ProviderId::Azure => (Implemented, "queue::azure_blob::AzureBlobBackend", "Preview Azure Blob Storage; not stable without release-scoped live acceptance"),
    ProviderId::Aws => (Implemented, "queue::s3::S3Backend", "Preview Amazon S3; not stable without release-scoped live acceptance"),
    ProviderId::Cloudflare => (Implemented, "queue::s3::vendor", "Cloudflare R2, endpoint from the account id; preview without live acceptance"),
    ProviderId::Backblaze => (Implemented, "queue::s3::vendor", "Backblaze B2 S3 API, endpoint from the region; preview without live acceptance"),
    ProviderId::Wasabi => (Implemented, "queue::s3::vendor", "Wasabi, endpoint from the region; preview without live acceptance"),
    ProviderId::DigitalOcean => (Implemented, "queue::s3::vendor", "DigitalOcean Spaces, endpoint from the region; preview without live acceptance"),
    ProviderId::Hetzner => (Implemented, "queue::s3::vendor", "Hetzner Object Storage, endpoint from the location; preview without live acceptance"),
    ProviderId::Scaleway => (Implemented, "queue::s3::vendor", "Scaleway Object Storage, endpoint from the region; preview without live acceptance"),
    ProviderId::Ovhcloud => (Implemented, "queue::s3::vendor", "OVHcloud Object Storage, endpoint from the region; preview without live acceptance"),
    ProviderId::Akamai => (Implemented, "queue::s3::vendor", "Akamai (Linode) Object Storage, endpoint from the region; preview without live acceptance"),
    ProviderId::Vultr => (Implemented, "queue::s3::vendor", "Vultr Object Storage, endpoint from the location; preview without live acceptance"),
    ProviderId::Oracle => (Implemented, "queue::s3::vendor", "OCI Object Storage through its S3 compatibility API, endpoint from the tenancy namespace and region; preview without live acceptance"),
    ProviderId::Ibm => (Implemented, "queue::s3::vendor", "IBM Cloud Object Storage, endpoint from the region; preview without live acceptance"),
    ProviderId::Alibaba => (Implemented, "queue::s3::vendor", "Alibaba Cloud OSS through its S3 API, endpoint from the region; preview without live acceptance"),
    ProviderId::Tencent => (Implemented, "queue::s3::vendor", "Tencent Cloud COS through its S3 API, endpoint from the region; preview without live acceptance"),
    ProviderId::Huawei => (Implemented, "queue::s3::vendor", "Huawei Cloud OBS through its S3 API, endpoint from the region; preview without live acceptance"),
    ProviderId::Exoscale => (Implemented, "queue::s3::vendor", "Exoscale Simple Object Storage, endpoint from the zone; preview without live acceptance"),
    ProviderId::Ionos => (Implemented, "queue::s3::vendor", "IONOS Object Storage, endpoint from the region; preview without live acceptance"),
    ProviderId::Contabo => (Implemented, "queue::s3::vendor", "Contabo Object Storage, endpoint from the region; preview without live acceptance"),
    ProviderId::UpCloud => (Implemented, "queue::s3::vendor", "UpCloud Managed Object Storage, endpoint from the instance and region; preview without live acceptance"),
    ProviderId::Storj => (Implemented, "queue::s3::vendor", "Storj through its S3-compatible gateway; preview without live acceptance"),
    ProviderId::Filebase => (Implemented, "queue::s3::vendor", "Filebase; preview without live acceptance"),
    ProviderId::Tigris => (Implemented, "queue::s3::vendor", "Tigris; preview without live acceptance"),
    ProviderId::Yandex => (Implemented, "queue::s3::vendor", "Yandex Object Storage; preview without live acceptance"),
    ProviderId::IdriveE2 => (Implemented, "queue::s3::vendor", "IDrive e2 at the endpoint the account was issued; preview without live acceptance"),
    ProviderId::Minio => (Implemented, "queue::s3::vendor", "Self-hosted MinIO at the endpoint the deployment runs; preview without live acceptance"),
    ProviderId::Garage => (Implemented, "queue::s3::vendor", "Self-hosted Garage at the endpoint the deployment runs; preview without live acceptance"),
    ProviderId::Seaweedfs => (Implemented, "queue::s3::vendor", "Self-hosted SeaweedFS S3 gateway at the endpoint the deployment runs; preview without live acceptance"),
    ProviderId::Ceph => (Implemented, "queue::s3::vendor", "Self-hosted Ceph Object Gateway at the endpoint the deployment runs; preview without live acceptance"),
    ProviderId::S3Compatible => (Implemented, "queue::s3::S3Backend::compatible", "Any other S3 API endpoint named by URL, with its region and addressing declared"),
    ProviderId::Local => (Implemented, "queue::local_file::LocalBackend", "Device-local filesystem"),
];

pub(super) const BACKUP_RECOVERY: &[ProviderCapability] = provider_rows![
    ProviderId::Stado => (Implemented, "queue::failover + queue::copy + cli::recovery", "Fenced migration and failover orchestration"),
    ProviderId::Gcp => (Implemented, "queue::gcs::GcsBackend", "GCS source or destination"),
    ProviderId::Azure => (Implemented, "queue::azure_blob::AzureBlobBackend", "Azure Blob source or destination"),
    ProviderId::Aws => (Implemented, "queue::s3::S3Backend", "S3 source or destination"),
    ProviderId::S3Compatible => (Implemented, "queue::s3::S3Backend::compatible + queue::s3::vendor", "Any S3-compatible store, named provider or endpoint URL, as source or destination"),
    ProviderId::Local => (Implemented, "queue::local_file::LocalBackend", "Filesystem source or destination"),
];
