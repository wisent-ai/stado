//! The object-storage providers that speak the S3 API, each named: where its
//! endpoint is, where it expects the bucket, and which region its requests
//! are signed for. Choosing `storage.backend cloudflare` (or `wasabi`,
//! `hetzner`, `minio`, …) asks only for what that provider's own endpoint is
//! built from; the generic `s3-compatible` backend remains for an endpoint no
//! profile names.
//!
//! Each endpoint template and fixed region is the provider's published S3
//! API contract, cited per profile: the same kind of declaration as a GPU
//! cloud vendor's API base (`providers::gpu_cloud::vendors`). A provider a
//! deployment runs itself (MinIO, Garage, SeaweedFS, Ceph) has no endpoint of
//! its own to publish, so its endpoint and region are the deployment's.

use crate::capabilities::ProviderId;

/// What a deployment declares for one provider, beyond its bucket and the
/// credential role holding its key pair. The capability registry reads it to
/// list the provider's configuration, and `endpoint::resolve` to build its
/// endpoint, so the two cannot ask for different things.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Declares {
    /// The account the provider's endpoint names.
    Account,
    /// The region the provider's endpoint names and requests are signed for.
    Region,
    /// Both.
    AccountAndRegion,
    /// Nothing more: one endpoint and one region serve every customer.
    Nothing,
    /// The endpoint and the region of a server the deployment runs.
    EndpointAndRegion,
}

mod endpoint;

pub use endpoint::{resolve, Reached};

/// Where a provider's endpoint comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointRule {
    /// The provider's published endpoint, with `{account}` and `{region}`
    /// filled from the deployment's declaration.
    Template(&'static str),
    /// A server the deployment runs or was issued: the endpoint is declared.
    Declared,
}

/// Which region a provider's requests are signed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionRule {
    /// The one region the provider documents for its S3 API.
    Fixed(&'static str),
    /// The region the deployment declares, as the provider names it.
    Declared,
}

/// One named S3 API provider.
#[derive(Clone, Copy, Debug)]
pub struct VendorProfile {
    pub provider: ProviderId,
    /// The provider's name for its object store.
    pub product: &'static str,
    pub endpoint: EndpointRule,
    /// `true` when the bucket goes in the path, `false` in the host name.
    pub path_style: bool,
    pub region: RegionRule,
    /// The provider's own description of its S3 API endpoint.
    pub reference: &'static str,
    /// What the deployment declares for it.
    pub declares: Declares,
}

macro_rules! storage_vendors {
    ($($vendor:ident),+ $(,)?) => {
        /// A named S3 API provider.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum StorageVendor {
            $($vendor,)+
        }

        impl StorageVendor {
            /// Every named provider, in catalog order.
            pub const ALL: &'static [Self] = &[$(Self::$vendor,)+];
        }
    };
}

storage_vendors!(
    CloudflareR2,
    BackblazeB2,
    Wasabi,
    DigitalOceanSpaces,
    Hetzner,
    Scaleway,
    Ovhcloud,
    Akamai,
    Vultr,
    Oracle,
    Ibm,
    Alibaba,
    Tencent,
    Huawei,
    Exoscale,
    Ionos,
    Contabo,
    UpCloud,
    Storj,
    Filebase,
    Tigris,
    Yandex,
    IdriveE2,
    Minio,
    Garage,
    Seaweedfs,
    Ceph,
);

const fn regional(
    provider: ProviderId,
    product: &'static str,
    template: &'static str,
    path_style: bool,
    reference: &'static str,
) -> VendorProfile {
    VendorProfile {
        provider,
        product,
        endpoint: EndpointRule::Template(template),
        path_style,
        region: RegionRule::Declared,
        reference,
        declares: Declares::Region,
    }
}

const fn self_hosted(
    provider: ProviderId,
    product: &'static str,
    reference: &'static str,
) -> VendorProfile {
    VendorProfile {
        provider,
        product,
        endpoint: EndpointRule::Declared,
        path_style: true,
        region: RegionRule::Declared,
        reference,
        declares: Declares::EndpointAndRegion,
    }
}

impl StorageVendor {
    pub const fn provider(self) -> ProviderId {
        self.profile().provider
    }

    /// The backend name `storage.backend` selects this provider by.
    pub const fn id(self) -> &'static str {
        self.provider().as_str()
    }

    pub const fn profile(self) -> VendorProfile {
        use ProviderId as P;
        match self {
            Self::CloudflareR2 => VendorProfile {
                provider: P::Cloudflare,
                product: "Cloudflare R2",
                endpoint: EndpointRule::Template("https://{account}.r2.cloudflarestorage.com"),
                path_style: true,
                region: RegionRule::Fixed("auto"),
                reference: "https://developers.cloudflare.com/r2/api/s3/api/",
                declares: Declares::Account,
            },
            Self::BackblazeB2 => regional(P::Backblaze, "Backblaze B2", "https://s3.{region}.backblazeb2.com", false, "https://www.backblaze.com/docs/cloud-storage-s3-compatible-api"),
            Self::Wasabi => regional(P::Wasabi, "Wasabi Hot Cloud Storage", "https://s3.{region}.wasabisys.com", false, "https://docs.wasabi.com/docs/what-are-the-service-urls-for-wasabi-s-different-storage-regions"),
            Self::DigitalOceanSpaces => regional(P::DigitalOcean, "DigitalOcean Spaces", "https://{region}.digitaloceanspaces.com", false, "https://docs.digitalocean.com/products/spaces/reference/s3-compatibility/"),
            Self::Hetzner => regional(P::Hetzner, "Hetzner Object Storage", "https://{region}.your-objectstorage.com", false, "https://docs.hetzner.com/storage/object-storage/overview/"),
            Self::Scaleway => regional(P::Scaleway, "Scaleway Object Storage", "https://s3.{region}.scw.cloud", false, "https://www.scaleway.com/en/docs/object-storage/api-cli/using-api-call-list/"),
            Self::Ovhcloud => regional(P::Ovhcloud, "OVHcloud Object Storage", "https://s3.{region}.io.cloud.ovh.net", false, "https://help.ovhcloud.com/csm/en-public-cloud-storage-s3-location"),
            Self::Akamai => regional(P::Akamai, "Akamai (Linode) Object Storage", "https://{region}.linodeobjects.com", false, "https://techdocs.akamai.com/cloud-computing/docs/object-storage"),
            Self::Vultr => regional(P::Vultr, "Vultr Object Storage", "https://{region}.vultrobjects.com", false, "https://docs.vultr.com/products/cloud-storage/object-storage"),
            Self::Oracle => VendorProfile {
                provider: P::Oracle,
                product: "OCI Object Storage (Amazon S3 compatibility API)",
                endpoint: EndpointRule::Template("https://{account}.compat.objectstorage.{region}.oraclecloud.com"),
                path_style: true,
                region: RegionRule::Declared,
                reference: "https://docs.oracle.com/en-us/iaas/Content/Object/Tasks/s3compatibleapi.htm",
                declares: Declares::AccountAndRegion,
            },
            Self::Ibm => regional(P::Ibm, "IBM Cloud Object Storage", "https://s3.{region}.cloud-object-storage.appdomain.cloud", false, "https://cloud.ibm.com/docs/cloud-object-storage?topic=cloud-object-storage-endpoints"),
            Self::Alibaba => regional(P::Alibaba, "Alibaba Cloud OSS", "https://s3.{region}.aliyuncs.com", false, "https://www.alibabacloud.com/help/en/oss/developer-reference/use-amazon-s3-sdks-to-access-oss"),
            Self::Tencent => regional(P::Tencent, "Tencent Cloud COS", "https://cos.{region}.myqcloud.com", false, "https://www.tencentcloud.com/document/product/436/32537"),
            Self::Huawei => regional(P::Huawei, "Huawei Cloud OBS", "https://obs.{region}.myhuaweicloud.com", false, "https://support.huaweicloud.com/intl/en-us/api-obs/obs_04_0001.html"),
            Self::Exoscale => regional(P::Exoscale, "Exoscale Simple Object Storage", "https://sos-{region}.exo.io", false, "https://community.exoscale.com/documentation/storage/s3-api-compatibility/"),
            Self::Ionos => regional(P::Ionos, "IONOS S3 Object Storage", "https://s3.{region}.ionoscloud.com", false, "https://docs.ionos.com/cloud/storage-and-backup/ionos-object-storage/endpoints"),
            Self::Contabo => regional(P::Contabo, "Contabo Object Storage", "https://{region}.contabostorage.com", true, "https://help.contabo.com/en/support/solutions/articles/103000270319"),
            Self::UpCloud => VendorProfile {
                provider: P::UpCloud,
                product: "UpCloud Managed Object Storage",
                endpoint: EndpointRule::Template("https://{account}.{region}.upcloudobjects.com"),
                path_style: true,
                region: RegionRule::Declared,
                reference: "https://upcloud.com/docs/products/managed-object-storage/",
                declares: Declares::AccountAndRegion,
            },
            Self::Storj => regional(P::Storj, "Storj (S3-compatible gateway)", "https://gateway.storjshare.io", true, "https://storj.dev/dcs/api/s3/s3-compatibility"),
            Self::Filebase => VendorProfile {
                provider: P::Filebase,
                product: "Filebase",
                endpoint: EndpointRule::Template("https://s3.filebase.com"),
                path_style: true,
                region: RegionRule::Fixed("us-east-1"),
                reference: "https://docs.filebase.com/api-documentation/s3-compatible-api",
                declares: Declares::Nothing,
            },
            Self::Tigris => VendorProfile {
                provider: P::Tigris,
                product: "Tigris",
                endpoint: EndpointRule::Template("https://t3.storage.dev"),
                path_style: false,
                region: RegionRule::Fixed("auto"),
                reference: "https://www.tigrisdata.com/docs/sdks/s3/",
                declares: Declares::Nothing,
            },
            Self::Yandex => VendorProfile {
                provider: P::Yandex,
                product: "Yandex Object Storage",
                endpoint: EndpointRule::Template("https://storage.yandexcloud.net"),
                path_style: true,
                region: RegionRule::Fixed("ru-central1"),
                reference: "https://yandex.cloud/en/docs/storage/s3/",
                declares: Declares::Nothing,
            },
            Self::IdriveE2 => VendorProfile {
                provider: P::IdriveE2,
                product: "IDrive e2",
                endpoint: EndpointRule::Declared,
                path_style: true,
                region: RegionRule::Declared,
                reference: "https://www.idrive.com/s3-storage-e2/faq-technical",
                declares: Declares::EndpointAndRegion,
            },
            Self::Minio => self_hosted(P::Minio, "MinIO", "https://min.io/docs/minio/linux/developers/minio-drivers.html"),
            Self::Garage => self_hosted(P::Garage, "Garage", "https://garagehq.deuxfleurs.fr/documentation/reference-manual/s3-compatibility/"),
            Self::Seaweedfs => self_hosted(P::Seaweedfs, "SeaweedFS S3 gateway", "https://github.com/seaweedfs/seaweedfs/wiki/Amazon-S3-API"),
            Self::Ceph => self_hosted(P::Ceph, "Ceph Object Gateway (RGW)", "https://docs.ceph.com/en/latest/radosgw/s3/"),
        }
    }
}
