//! Single-source catalog of Stado's user-facing capabilities and their provider
//! support, plus the internal adapter/configuration facets that implement them.
//!
//! [`CAPABILITIES`] answers what a user can ask Stado to provide and which
//! providers implement, partially support, expose externally, or plan that
//! feature. [`REGISTRY`] is deliberately narrower: it routes existing runtime
//! adapters and is not a second product capability list. Provider names are
//! declared exactly once by [`ProviderId`].

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

macro_rules! define_providers {
    ($($variant:ident => ($id:literal, [$($alias:literal),* $(,)?])),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum ProviderId {
            $($variant,)+
        }

        impl ProviderId {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)+
                }
            }

            pub const fn aliases(self) -> &'static [&'static str] {
                match self {
                    $(Self::$variant => &[$($alias),*],)+
                }
            }
        }

        impl Serialize for ProviderId {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        pub const PROVIDERS: &[ProviderId] = &[
            $(ProviderId::$variant,)+
        ];
    };
}

define_providers! {
    Gcp => ("gcp", []),
    Azure => ("azure", []),
    Aws => ("aws", []),
    Box => ("box", ["box-ascii"]),
    Local => ("local", []),
    Vast => ("vast", []),
    Stado => ("stado", []),
    Huggingface => ("huggingface", []),
    Skarbiec => ("skarbiec", []),
    Supabase => ("supabase", []),
    Slack => ("slack", []),
    Telegram => ("telegram", []),
    Sendgrid => ("sendgrid", []),
    Most => ("most", []),
    Macos => ("macos", []),
    Linux => ("linux", []),
    MultiCloud => ("multi-cloud", []),
    ProviderNeutral => ("provider-neutral", []),
}

impl ProviderId {
    pub fn matches(self, raw: &str) -> bool {
        provider(raw) == Some(self)
    }

    pub const fn inventory_limitation(self) -> Option<&'static str> {
        match self {
            Self::Aws => Some(
                "AWS: agent VM inventory is complete; EBS, Elastic IPs, reservations, non-Stado resources, and AWS cost data are not enumerated",
            ),
            Self::Azure => Some(
                "Azure: agent VM inventory is complete; managed disks, public IPs, reservations, and non-Stado resources are not enumerated",
            ),
            Self::Box => Some(
                "Box: externally owned marketplace capacity has no standing VM inventory",
            ),
            _ => None,
        }
    }

    pub fn infer_from_instance_reference(reference: &str) -> Option<Self> {
        if reference.starts_with("local@") {
            return Some(Self::Local);
        }
        let (_, location) = reference.rsplit_once('@')?;
        let mut zone = location.rsplit('-').next()?.chars();
        let suffix = zone.next()?;
        (zone.next().is_none() && suffix.is_ascii_alphabetic()).then_some(Self::Gcp)
    }

    pub fn owns_release_url(self, url: &str) -> bool {
        match self {
            Self::Gcp => url.contains("googleapis.com") || url.contains("storage.cloud.google.com"),
            Self::Azure => url.contains("blob.core.windows.net"),
            Self::Aws => url.contains("amazonaws.com"),
            Self::Local => url.starts_with("file:"),
            _ => false,
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ProviderId {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        provider(raw).ok_or_else(|| format!("unknown provider {raw:?}"))
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        provider(&raw).ok_or_else(|| serde::de::Error::custom(format!("unknown provider {raw:?}")))
    }
}

pub fn provider(name: &str) -> Option<ProviderId> {
    PROVIDERS
        .iter()
        .copied()
        .find(|provider| provider.as_str() == name || provider.aliases().contains(&name))
}

/// User-facing, provider-independent feature support.
///
/// `Implemented` means Stado owns an operational adapter. `Partial` means the
/// user-facing contract is narrower than the capability description.
/// `External` records a dependency Stado can inspect or consume but does not
/// manage. `Planned` names a known provider equivalent without pretending that
/// an adapter exists. An omitted provider is `Unsupported`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilitySupport {
    Implemented,
    Partial,
    External,
    Planned,
    Unsupported,
}

impl CapabilitySupport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::Partial => "partial",
            Self::External => "external",
            Self::Planned => "planned",
            Self::Unsupported => "unsupported",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderCapability {
    pub provider: ProviderId,
    pub support: CapabilitySupport,
    pub implementation: &'static str,
    pub note: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProductCapability {
    pub id: CapabilityKind,
    pub summary: &'static str,
    pub providers: &'static [ProviderCapability],
}

impl ProductCapability {
    pub fn support(self, provider: ProviderId) -> CapabilitySupport {
        self.providers
            .iter()
            .find(|entry| entry.provider == provider)
            .map(|entry| entry.support)
            .unwrap_or(CapabilitySupport::Unsupported)
    }
}

macro_rules! define_capabilities {
    (
        $(
            $variant:ident => {
                id: $id:literal,
                summary: $summary:literal,
                providers: [
                    $(
                        $provider:path => (
                            $support:ident,
                            $implementation:literal,
                            $note:literal
                        )
                    ),* $(,)?
                ]
            }
        ),+ $(,)?
    ) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(rename_all = "kebab-case")]
        pub enum CapabilityKind {
            $($variant,)+
        }

        impl CapabilityKind {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)+
                }
            }
        }

        impl fmt::Display for CapabilityKind {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for CapabilityKind {
            type Err = String;

            fn from_str(raw: &str) -> Result<Self, Self::Err> {
                CAPABILITIES
                    .iter()
                    .find(|capability| capability.id.as_str() == raw)
                    .map(|capability| capability.id)
                    .ok_or_else(|| format!("unknown capability {raw:?}"))
            }
        }

        /// Complete product-level capability catalog. This macro invocation is
        /// the only declaration of capability ids, descriptions, and provider
        /// support; enum, lookup, serialization, and CLI views derive from it.
        pub static CAPABILITIES: &[ProductCapability] = &[
            $(
                ProductCapability {
                    id: CapabilityKind::$variant,
                    summary: $summary,
                    providers: &[
                        $(
                            ProviderCapability {
                                provider: $provider,
                                support: CapabilitySupport::$support,
                                implementation: $implementation,
                                note: $note,
                            },
                        )*
                    ],
                },
            )+
        ];
    };
}

define_capabilities! {
    Compute => {
        id: "compute",
        summary: "Provision or attach CPU and accelerator-backed machines.",
        providers: [
            ProviderId::Gcp => (Partial, "providers::gcp::GcpProvider", "Preview Google Compute Engine VM lifecycle; not stable without release-scoped live acceptance"),
            ProviderId::Azure => (Implemented, "providers::azure::AzureProvider", "Preview Azure Virtual Machines lifecycle; not stable without release-scoped live acceptance"),
            ProviderId::Aws => (Implemented, "providers::aws::AwsProvider", "Preview Amazon EC2 lifecycle; not stable without release-scoped live acceptance"),
            ProviderId::Box => (Implemented, "providers::box::BoxProvider", "Externally managed fixed-shape boxes"),
            ProviderId::Local => (Partial, "providers::local", "Attach existing hosts; no machine provisioning"),
            ProviderId::Vast => (Partial, "providers::vast", "Publish a host; renter provisioning is not implemented"),
        ]
    },
    ManagedCompute => {
        id: "managed-compute",
        summary: "Operate autoscaled or provider-managed groups of machines.",
        providers: [
            ProviderId::Gcp => (Partial, "cli::resources", "Managed instance groups and templates are inspected and selected mutations are supported"),
            ProviderId::Azure => (Planned, "", "Azure Virtual Machine Scale Sets equivalent; no Stado adapter"),
            ProviderId::Aws => (Planned, "", "Amazon EC2 Auto Scaling equivalent; no Stado adapter"),
            ProviderId::Local => (External, "targets + coordinator", "Local host pools are scheduled but not autoscaled"),
        ]
    },
    WorkloadExecution => {
        id: "workload-execution",
        summary: "Run a queued workload in a managed agent environment.",
        providers: [
            ProviderId::Stado => (Implemented, "scheduler + agent", "Provider-neutral queue, leases, dispatch, and execution contract"),
            ProviderId::Gcp => (Partial, "providers::local::agent + providers::gcp", "Preview ephemeral agent lifecycle on an owned GCE VM"),
            ProviderId::Azure => (Implemented, "providers::local::agent + providers::azure", "Preview ephemeral agent lifecycle on an Azure VM"),
            ProviderId::Aws => (Implemented, "providers::local::agent + providers::aws", "Preview ephemeral agent lifecycle on an EC2 VM"),
            ProviderId::Box => (Implemented, "providers::local::agent + providers::box", "Agent bootstrapped on a leased box"),
            ProviderId::Local => (Implemented, "providers::local::agent", "Long-lived workstation or server agent"),
            ProviderId::Vast => (Partial, "providers::local::agent + providers::vast", "Agent execution on an operator-published Vast host; renter provisioning is unavailable"),
        ]
    },
    ObjectStorage => {
        id: "object-storage",
        summary: "Persist queue state, results, artifacts, and control objects.",
        providers: [
            ProviderId::Stado => (Implemented, "queue::stado_object::StadoObjectBackend + dashboard object API", "Authenticated provider-neutral shared queue over HTTPS"),
            ProviderId::Gcp => (Implemented, "queue::gcs::GcsBackend", "Preview Google Cloud Storage; not stable without release-scoped live acceptance"),
            ProviderId::Azure => (Implemented, "queue::azure_blob::AzureBlobBackend", "Preview Azure Blob Storage; not stable without release-scoped live acceptance"),
            ProviderId::Aws => (Implemented, "queue::s3::S3Backend", "Preview Amazon S3; not stable without release-scoped live acceptance"),
            ProviderId::Local => (Implemented, "queue::local_file::LocalBackend", "Device-local filesystem"),
        ]
    },
    BlockStorage => {
        id: "block-storage",
        summary: "Attach and manage machine boot or persistent block devices.",
        providers: [
            ProviderId::Gcp => (Partial, "providers::gcp + cli::resources", "Boot disks and selected Compute Engine disk operations"),
            ProviderId::Azure => (Partial, "providers::azure", "VM operating-system disk provisioning"),
            ProviderId::Aws => (Partial, "providers::aws", "EC2 root-volume provisioning"),
            ProviderId::Local => (External, "host operating system", "Local disks are consumed but not provisioned by Stado"),
        ]
    },
    MachineImages => {
        id: "machine-images",
        summary: "Select immutable images used to bootstrap workload machines.",
        providers: [
            ProviderId::Gcp => (Implemented, "providers::gcp", "Compute Engine image projects and families"),
            ProviderId::Azure => (Implemented, "providers::azure", "Azure image URNs"),
            ProviderId::Aws => (Implemented, "providers::aws", "Amazon Machine Images"),
            ProviderId::Local => (External, "host operating system", "Existing host installation is managed outside machine provisioning"),
        ]
    },
    ContainerRegistry => {
        id: "container-registry",
        summary: "Store and retrieve versioned container images.",
        providers: [
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Artifact Registry is inventoried but not managed by a Stado adapter"),
            ProviderId::Azure => (Planned, "", "Azure Container Registry equivalent; no Stado adapter"),
            ProviderId::Aws => (Planned, "", "Amazon Elastic Container Registry equivalent; no Stado adapter"),
            ProviderId::Local => (External, "container runtime", "Local image storage is owned by the installed container runtime"),
        ]
    },
    ApplicationHosting => {
        id: "application-hosting",
        summary: "Run a continuously available service or control plane.",
        providers: [
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Cloud Run services are inventoried; provisioning is external"),
            ProviderId::Azure => (Planned, "", "No Azure application-hosting adapter"),
            ProviderId::Aws => (Planned, "", "No AWS application-hosting adapter"),
            ProviderId::Local => (Implemented, "deploy::service", "launchd and systemd service lifecycle"),
        ]
    },
    ServerlessFunctions => {
        id: "serverless-functions",
        summary: "Execute an event-driven or interval-driven stateless function.",
        providers: [
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Legacy Cloud Function is observable but retired from the active control plane"),
            ProviderId::Azure => (Planned, "", "Azure Functions equivalent; no Stado adapter"),
            ProviderId::Aws => (Planned, "", "AWS Lambda equivalent; no Stado adapter"),
        ]
    },
    Scheduling => {
        id: "scheduling",
        summary: "Assign queued work and trigger recurring operations.",
        providers: [
            ProviderId::Stado => (Implemented, "scheduler + schedules", "Makespan assignment and recurring schedules"),
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Cloud Scheduler is inventoried but not the active scheduler"),
            ProviderId::Local => (Implemented, "control_plane::local", "Long-running local coordinator"),
        ]
    },
    Messaging => {
        id: "messaging",
        summary: "Publish asynchronous events and user notifications.",
        providers: [
            ProviderId::Gcp => (Partial, "monitor::alerts", "Pub/Sub publication for alerts"),
            ProviderId::Slack => (Implemented, "monitor::alerts", "Slack webhook delivery"),
            ProviderId::Telegram => (Implemented, "monitor::alerts", "Telegram Bot API delivery"),
            ProviderId::Sendgrid => (Implemented, "monitor::alerts", "SendGrid email delivery"),
            ProviderId::Azure => (Planned, "", "No Azure messaging adapter"),
            ProviderId::Aws => (Planned, "", "No AWS messaging adapter"),
        ]
    },
    DataAnalytics => {
        id: "data-analytics",
        summary: "Query operational datasets for usage and product insights.",
        providers: [
            ProviderId::Gcp => (Partial, "monitor::billing", "BigQuery queries are implemented for billing exports"),
            ProviderId::Azure => (Planned, "", "No general Azure analytics adapter"),
            ProviderId::Aws => (Planned, "", "No general AWS analytics adapter"),
            ProviderId::Local => (External, "local tools", "No shared Stado analytics service"),
        ]
    },
    Networking => {
        id: "networking",
        summary: "Provide network placement, addressing, and access boundaries.",
        providers: [
            ProviderId::Gcp => (Partial, "providers::gcp::inventory + cli::resources", "Networks, firewall rules, and addresses are inspected; selected addresses are managed"),
            ProviderId::Azure => (External, "providers::azure", "Pre-provisioned VNet, subnet, and network security group"),
            ProviderId::Aws => (External, "providers::aws", "Pre-provisioned security group and account networking"),
            ProviderId::Local => (External, "host operating system", "Host networking is consumed but not provisioned"),
        ]
    },
    LoadBalancing => {
        id: "load-balancing",
        summary: "Expose services through stable health-checked endpoints.",
        providers: [
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Historical backend services, health checks, and forwarding rules"),
            ProviderId::Azure => (Planned, "", "No Azure load-balancing adapter"),
            ProviderId::Aws => (Planned, "", "No AWS load-balancing adapter"),
            ProviderId::Local => (External, "deployment environment", "Reverse proxy and local routing are managed outside Stado"),
        ]
    },
    IdentityAccess => {
        id: "identity-access",
        summary: "Authenticate workloads and authorize provider operations.",
        providers: [
            ProviderId::Skarbiec => (Implemented, "dashboard authorization + skarbiec::Client", "Scoped bearer resolution for Stado APIs"),
            ProviderId::Gcp => (Implemented, "skarbiec::gcp_provider", "Application Default Credentials and workload identity"),
            ProviderId::Azure => (Implemented, "azure_token", "Managed identity and operator token chain"),
            ProviderId::Aws => (Implemented, "providers::aws::sdk_config", "AWS credential chain, IMDS, and scoped fallback"),
            ProviderId::Local => (Partial, "deploy::host_channel", "Local account and SSH identity"),
            ProviderId::Supabase => (Implemented, "dashboard::authorized", "Optional dashboard user JWT authorization through RLS; not required by the core control plane"),
        ]
    },
    Secrets => {
        id: "secrets",
        summary: "Store and deliver scoped application or workload secrets.",
        providers: [
            ProviderId::Skarbiec => (Implemented, "skarbiec::Client", "Canonical scoped Stado secret service"),
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Historical Secret Manager assets are inventoried"),
            ProviderId::Azure => (Planned, "", "No Azure Key Vault adapter"),
            ProviderId::Aws => (Planned, "", "No AWS Secrets Manager adapter"),
            ProviderId::Local => (Implemented, "skarbiec::Client", "Local consumers use scoped Skarbiec grants"),
        ]
    },
    Build => {
        id: "build",
        summary: "Build reproducible binaries, images, or deployable service artifacts.",
        providers: [
            ProviderId::Gcp => (External, "cloudbuild.yaml + providers::gcp::inventory", "Cloud Build configuration exists; execution is external"),
            ProviderId::Azure => (Planned, "", "No Azure build adapter"),
            ProviderId::Aws => (Planned, "", "No AWS build adapter"),
            ProviderId::Local => (Implemented, "deploy::host_build_cache", "Host-local builds and build-cache management"),
        ]
    },
    Observability => {
        id: "observability",
        summary: "Inspect health, logs, heartbeats, failures, and operational state.",
        providers: [
            ProviderId::Stado => (Implemented, "overview + doctor + monitor + watchdog", "Provider-neutral operational view and health evaluation"),
            ProviderId::Gcp => (Partial, "providers::gcp::inventory", "Fault-isolated GCP resource probes"),
            ProviderId::Azure => (Partial, "cli::resources::inventory", "Stado-owned VM and billing health"),
            ProviderId::Aws => (Partial, "cli::resources::inventory", "Stado-owned EC2 inventory"),
            ProviderId::Local => (Implemented, "monitor::host_health", "Registry beacons and local service health"),
        ]
    },
    Inventory => {
        id: "inventory",
        summary: "Enumerate resources and workers owned or consumed by Stado.",
        providers: [
            ProviderId::Gcp => (Implemented, "providers::gcp::inventory", "Compute, storage, IAM, network, and managed-service assets"),
            ProviderId::Azure => (Partial, "cli::resources::inventory", "Stado-owned Azure agent VMs"),
            ProviderId::Aws => (Partial, "cli::resources::inventory", "Stado-owned EC2 agent VMs"),
            ProviderId::Local => (Implemented, "targets + monitor::host_health", "Registered hosts, services, and capacity beacons"),
            ProviderId::Box => (Partial, "providers::box", "Leased box lifecycle and account limits"),
            ProviderId::Vast => (Partial, "providers::vast", "Published host and marketplace state"),
        ]
    },
    QuotaCapacity => {
        id: "quota-capacity",
        summary: "Report allocatable capacity, quotas, and reservations.",
        providers: [
            ProviderId::Stado => (Implemented, "config/quotas.json + queue::capacity", "Provider-neutral reservations and published capacity"),
            ProviderId::Gcp => (Partial, "scheduler::quota", "Preview live accelerator quota reads plus configured reservations"),
            ProviderId::Azure => (Partial, "scheduler::quota", "Configured reservations; live VM-family coverage is incomplete"),
            ProviderId::Aws => (Planned, "", "No live AWS quota adapter"),
            ProviderId::Local => (Implemented, "queue::capacity + providers::local", "GPU probe, free VRAM, and agent slots"),
            ProviderId::Box => (Implemented, "providers::box", "Account limits and available boxes"),
            ProviderId::Vast => (Partial, "providers::vast", "Published host capacity"),
        ]
    },
    BillingCost => {
        id: "billing-cost",
        summary: "Estimate prices and monitor spend, credits, and billing health.",
        providers: [
            ProviderId::Gcp => (Implemented, "scheduler::cost + monitor::billing", "Machine prices, BigQuery export, credits, budgets, and burn"),
            ProviderId::Azure => (Implemented, "scheduler::cost + monitor::billing", "Machine prices, balance, usage, and billing health"),
            ProviderId::Aws => (Partial, "scheduler::cost", "Machine-price estimation without live billing-health collection"),
            ProviderId::Box => (Partial, "providers::box", "Lease cost is provider-owned"),
            ProviderId::Local => (External, "operator", "No cloud bill; hardware cost is outside Stado"),
        ]
    },
    ArtifactDistribution => {
        id: "artifact-distribution",
        summary: "Publish, verify, and retrieve immutable artifacts and releases.",
        providers: [
            ProviderId::Stado => (Implemented, "artifacts + self_update", "Manifest registry and signed HTTPS release channel"),
            ProviderId::Huggingface => (Implemented, "artifacts::adapters::ActivationDatasetAdapter", "Activation-dataset verification"),
            ProviderId::Gcp => (External, "queue::gcs", "Historical GCS release origin"),
            ProviderId::Azure => (Partial, "self_update + queue::azure_blob", "Azure-hosted release reads and object storage"),
            ProviderId::Aws => (Planned, "", "No dedicated S3 release publisher adapter"),
            ProviderId::Local => (Partial, "artifacts + filesystem", "Local artifact manifests and files"),
        ]
    },
    BackupRecovery => {
        id: "backup-recovery",
        summary: "Mirror, copy, verify, and recover provider-neutral state.",
        providers: [
            ProviderId::Stado => (Implemented, "queue::failover + queue::copy + cli::recovery", "Fenced migration and failover orchestration"),
            ProviderId::Gcp => (Implemented, "queue::gcs::GcsBackend", "GCS source or destination"),
            ProviderId::Azure => (Implemented, "queue::azure_blob::AzureBlobBackend", "Azure Blob source or destination"),
            ProviderId::Aws => (Implemented, "queue::s3::S3Backend", "S3 source or destination"),
            ProviderId::Local => (Implemented, "queue::local_file::LocalBackend", "Filesystem source or destination"),
        ]
    },
}

pub fn product_capabilities() -> &'static [ProductCapability] {
    CAPABILITIES
}

pub fn product_capability(id: &str) -> Option<&'static ProductCapability> {
    CAPABILITIES
        .iter()
        .find(|capability| capability.id.as_str() == id)
}

pub fn capability_support(capability: CapabilityKind, provider: ProviderId) -> CapabilitySupport {
    CAPABILITIES
        .iter()
        .find(|entry| entry.id == capability)
        .map(|entry| entry.support(provider))
        .unwrap_or(CapabilitySupport::Unsupported)
}

pub fn capabilities_for_provider(
    provider: ProviderId,
) -> impl Iterator<Item = &'static ProductCapability> {
    CAPABILITIES
        .iter()
        .filter(move |capability| capability.support(provider) != CapabilitySupport::Unsupported)
}

/// Internal routing facets retained separately from user-facing capabilities.
/// These values organize adapters and configuration; they are not the product
/// capability list returned by [`product_capabilities`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeFacet {
    Compute,
    Storage,
    Execution,
    Scheduling,
    Inventory,
    Quota,
    Billing,
    Artifacts,
    Authentication,
    Secrets,
    Alerts,
    Deployment,
    HostTarget,
    Dependency,
}

impl RuntimeFacet {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compute => "compute",
            Self::Storage => "storage",
            Self::Execution => "execution",
            Self::Scheduling => "scheduling",
            Self::Inventory => "inventory",
            Self::Quota => "quota",
            Self::Billing => "billing",
            Self::Artifacts => "artifacts",
            Self::Authentication => "authentication",
            Self::Secrets => "secrets",
            Self::Alerts => "alerts",
            Self::Deployment => "deployment",
            Self::HostTarget => "host-target",
            Self::Dependency => "dependency",
        }
    }

    pub const fn product_capability(self) -> CapabilityKind {
        match self {
            Self::Compute => CapabilityKind::Compute,
            Self::Storage => CapabilityKind::ObjectStorage,
            Self::Execution => CapabilityKind::WorkloadExecution,
            Self::Scheduling => CapabilityKind::Scheduling,
            Self::Inventory => CapabilityKind::Inventory,
            Self::Quota => CapabilityKind::QuotaCapacity,
            Self::Billing => CapabilityKind::BillingCost,
            Self::Artifacts => CapabilityKind::ArtifactDistribution,
            Self::Authentication => CapabilityKind::IdentityAccess,
            Self::Secrets => CapabilityKind::Secrets,
            Self::Alerts => CapabilityKind::Messaging,
            Self::Deployment => CapabilityKind::ApplicationHosting,
            Self::HostTarget => CapabilityKind::Inventory,
            Self::Dependency => CapabilityKind::BackupRecovery,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionMode {
    Single,
    OrderedMany,
    ConcurrentMany,
    Automatic,
    Internal,
}

impl SelectionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::OrderedMany => "ordered-many",
            Self::ConcurrentMany => "concurrent-many",
            Self::Automatic => "automatic",
            Self::Internal => "internal",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeAdapter {
    Gcp,
    Azure,
    Aws,
    Box,
    ExistingHost,
    VastHost,
}

impl ComputeAdapter {
    pub const fn tracks_cloud_cost(self) -> bool {
        matches!(self, Self::Gcp | Self::Azure | Self::Aws)
    }

    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
            Self::Box => ProviderId::Box,
            Self::ExistingHost => ProviderId::Local,
            Self::VastHost => ProviderId::Vast,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageAdapter {
    Gcs,
    AzureBlob,
    S3,
    StadoObject,
    Local,
}

impl StorageAdapter {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Gcs => "gcs",
            Self::AzureBlob => ProviderId::Azure.as_str(),
            Self::S3 => "s3",
            Self::StadoObject => ProviderId::Stado.as_str(),
            Self::Local => ProviderId::Local.as_str(),
        }
    }

    pub const fn required_backup(self) -> Option<Self> {
        match self {
            Self::AzureBlob => Some(Self::S3),
            _ => None,
        }
    }

    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcs => ProviderId::Gcp,
            Self::AzureBlob => ProviderId::Azure,
            Self::S3 => ProviderId::Aws,
            Self::StadoObject => ProviderId::Stado,
            Self::Local => ProviderId::Local,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionAdapter {
    Local,
    Gcp,
    Azure,
    Aws,
    Box,
    Vast,
}

impl ExecutionAdapter {
    pub const fn allows_job_system_packages(self) -> bool {
        !matches!(self, Self::Local)
    }

    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Local => ProviderId::Local,
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
            Self::Box => ProviderId::Box,
            Self::Vast => ProviderId::Vast,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryAdapter {
    Gcp,
    Azure,
    Aws,
}

impl InventoryAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaAdapter {
    Gcp,
    Azure,
    StorageOverlay,
}

impl QuotaAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::StorageOverlay => ProviderId::Stado,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BillingAdapter {
    Gcp,
    Azure,
}

impl BillingAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencyAdapter {
    Gcp,
    Azure,
    Aws,
    Local,
}

impl DependencyAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
            Self::Local => ProviderId::Local,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeAdapter {
    None,
    Compute(ComputeAdapter),
    Storage(StorageAdapter),
    Execution(ExecutionAdapter),
    Inventory(InventoryAdapter),
    Quota(QuotaAdapter),
    Billing(BillingAdapter),
    Dependency(DependencyAdapter),
}

impl RuntimeAdapter {
    pub const fn provider(self) -> Option<ProviderId> {
        match self {
            Self::None => None,
            Self::Compute(adapter) => Some(adapter.provider()),
            Self::Storage(adapter) => Some(adapter.provider()),
            Self::Execution(adapter) => Some(adapter.provider()),
            Self::Inventory(adapter) => Some(adapter.provider()),
            Self::Quota(adapter) => Some(adapter.provider()),
            Self::Billing(adapter) => Some(adapter.provider()),
            Self::Dependency(adapter) => Some(adapter.provider()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigValueKind {
    Scalar,
    List,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConfigField {
    pub key: &'static str,
    pub env: &'static str,
    pub path: &'static str,
    pub value_kind: ConfigValueKind,
    pub fallback_path: Option<&'static str>,
    pub fallback_env: Option<&'static str>,
    pub backup_path: Option<&'static str>,
    pub backup_env: Option<&'static str>,
    pub required: bool,
    pub backup_required: bool,
}

impl ConfigField {
    pub const fn scalar(key: &'static str, env: &'static str, path: &'static str) -> Self {
        Self {
            key,
            env,
            path,
            value_kind: ConfigValueKind::Scalar,
            fallback_path: None,
            fallback_env: None,
            backup_path: None,
            backup_env: None,
            required: false,
            backup_required: false,
        }
    }

    pub const fn list(key: &'static str, env: &'static str, path: &'static str) -> Self {
        Self {
            value_kind: ConfigValueKind::List,
            ..Self::scalar(key, env, path)
        }
    }

    pub const fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub const fn with_fallback(
        mut self,
        env: Option<&'static str>,
        path: Option<&'static str>,
    ) -> Self {
        self.fallback_env = env;
        self.fallback_path = path;
        self
    }

    pub const fn with_backup(
        mut self,
        env: &'static str,
        path: &'static str,
        required: bool,
    ) -> Self {
        self.backup_env = Some(env);
        self.backup_path = Some(path);
        self.backup_required = required;
        self
    }
}

pub const PROVIDERS_CONFIG: ConfigField =
    ConfigField::list("providers", "WC_PROVIDERS", "providers").required();

pub const DISABLED_PROVIDERS_CONFIG: ConfigField = ConfigField::list(
    "providers-disabled",
    "WC_DISABLED_PROVIDERS",
    "providers_disabled",
);

pub const STORAGE_BACKEND_CONFIG: ConfigField =
    ConfigField::scalar("backend", "WC_STORAGE_BACKEND", "storage.backend")
        .required()
        .with_backup("WC_BACKUP_STORAGE_BACKEND", "storage.backup.backend", false);

pub const CONTROL_CONFIG: &[ConfigField] = &[
    PROVIDERS_CONFIG,
    DISABLED_PROVIDERS_CONFIG,
    STORAGE_BACKEND_CONFIG,
];

const BACKUP_BUCKET_ENV: &str = "WC_BACKUP_BUCKET";
const BACKUP_BUCKET_PATH: &str = "storage.backup.bucket";
const AWS_REGION_CONFIG: ConfigField = ConfigField::scalar("region", "AWS_REGION", "aws.region");

const GCP_COMPUTE_CONFIG: &[ConfigField] = &[
    ConfigField::scalar("project", "GCP_PROJECT", "project").required(),
    ConfigField::scalar("region", "GCP_REGION", "region"),
    ConfigField::list("regions", "GCP_REGIONS", "regions"),
];

const AZURE_COMPUTE_CONFIG: &[ConfigField] = &[
    ConfigField::scalar(
        "subscription-id",
        "AZURE_SUBSCRIPTION_ID",
        "azure.subscription_id",
    )
    .required(),
    ConfigField::scalar(
        "resource-group",
        "AZURE_RESOURCE_GROUP",
        "azure.resource_group",
    ),
    ConfigField::list("locations", "AZURE_LOCATIONS", "azure.locations"),
    ConfigField::scalar("vnet", "AZURE_VNET", "azure.vnet"),
    ConfigField::scalar("subnet", "AZURE_SUBNET", "azure.subnet"),
    ConfigField::scalar("nsg", "AZURE_NSG", "azure.nsg"),
    ConfigField::scalar("image-urn", "AZURE_IMAGE_URN", "azure.image_urn"),
    ConfigField::scalar("vm-username", "AZURE_VM_USERNAME", "azure.vm_username"),
    ConfigField::scalar(
        "vm-identity-id",
        "AZURE_VM_IDENTITY_ID",
        "azure.vm_identity_id",
    )
    .required(),
    ConfigField::scalar(
        "ssh-public-key",
        "AZURE_SSH_PUBLIC_KEY",
        "azure.ssh_public_key",
    )
    .required(),
];

const AWS_COMPUTE_CONFIG: &[ConfigField] = &[
    AWS_REGION_CONFIG,
    ConfigField::scalar("security-group", "AWS_SECURITY_GROUP", "aws.security_group").required(),
    ConfigField::scalar("iam-profile", "AWS_IAM_PROFILE", "aws.iam_profile"),
    ConfigField::scalar("ami-id", "AWS_AMI_ID", "aws.ami_id"),
];

const GCS_CONFIG: &[ConfigField] =
    &[
        ConfigField::scalar("bucket", "WC_BUCKET", "storage.gcs.bucket")
            .required()
            .with_fallback(None, Some("bucket"))
            .with_backup(BACKUP_BUCKET_ENV, BACKUP_BUCKET_PATH, true),
    ];

const AZURE_STORAGE_CONFIG: &[ConfigField] = &[
    ConfigField::scalar(
        "account",
        "WC_AZURE_STORAGE_ACCOUNT",
        "storage.azure.account",
    )
    .required()
    .with_backup(
        "WC_BACKUP_AZURE_STORAGE_ACCOUNT",
        "storage.backup.azure.account",
        true,
    ),
    ConfigField::scalar("container", "WC_AZURE_CONTAINER", "storage.azure.container")
        .required()
        .with_backup(
            "WC_BACKUP_AZURE_CONTAINER",
            "storage.backup.azure.container",
            true,
        ),
];

const S3_CONFIG: &[ConfigField] = &[
    ConfigField::scalar("bucket", "WC_S3_BUCKET", "storage.s3.bucket")
        .required()
        .with_backup(BACKUP_BUCKET_ENV, BACKUP_BUCKET_PATH, true),
    ConfigField::scalar("region", "WC_S3_REGION", "storage.s3.region")
        .with_fallback(Some(AWS_REGION_CONFIG.env), Some(AWS_REGION_CONFIG.path))
        .with_backup("WC_BACKUP_S3_REGION", "storage.backup.s3.region", true),
];

const STADO_OBJECT_STORAGE_CONFIG: &[ConfigField] = &[
    ConfigField::scalar("url", "WC_STADO_STORAGE_URL", "storage.stado.url").required(),
    ConfigField::scalar(
        "token-file",
        "WC_STADO_STORAGE_TOKEN_FILE",
        "storage.stado.token_file",
    )
    .required(),
    ConfigField::scalar(
        "namespace",
        "WC_STADO_STORAGE_NAMESPACE",
        "storage.stado.namespace",
    )
    .required(),
];

const LOCAL_STORAGE_CONFIG: &[ConfigField] =
    &[
        ConfigField::scalar("path", "WC_LOCAL_STORAGE_PATH", "storage.local.path").with_backup(
            "WC_BACKUP_LOCAL_STORAGE_PATH",
            "storage.backup.local.path",
            true,
        ),
    ];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CapabilityVariant {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub provider: Option<ProviderId>,
    pub implementation: &'static str,
    pub summary: &'static str,
    pub configurable: bool,
    pub constructible: bool,
    #[serde(skip)]
    pub adapter: RuntimeAdapter,
    #[serde(skip)]
    pub config: &'static [ConfigField],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Capability {
    pub kind: RuntimeFacet,
    pub selection: SelectionMode,
    pub summary: &'static str,
    pub variants: &'static [CapabilityVariant],
}

const COMPUTE: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "providers::gcp::GcpProvider",
        summary: "Provision and reap Google Compute Engine instances.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Compute(ComputeAdapter::Gcp),
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Azure.as_str(),
        aliases: ProviderId::Azure.aliases(),
        provider: Some(ProviderId::Azure),
        implementation: "providers::azure::AzureProvider",
        summary: "Provision and reap Azure virtual machines.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Compute(ComputeAdapter::Azure),
        config: AZURE_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Aws.as_str(),
        aliases: ProviderId::Aws.aliases(),
        provider: Some(ProviderId::Aws),
        implementation: "providers::aws::AwsProvider",
        summary: "Provision and reap Amazon EC2 instances.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Compute(ComputeAdapter::Aws),
        config: AWS_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Box.as_str(),
        aliases: ProviderId::Box.aliases(),
        provider: Some(ProviderId::Box),
        implementation: "providers::box::BoxProvider",
        summary: "Lease externally managed fixed-shape boxes.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Compute(ComputeAdapter::Box),
        config: &[],
    },
    CapabilityVariant {
        id: ProviderId::Local.as_str(),
        aliases: ProviderId::Local.aliases(),
        provider: Some(ProviderId::Local),
        implementation: "providers::local",
        summary: "Execute on an existing local host without VM provisioning.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Compute(ComputeAdapter::ExistingHost),
        config: &[],
    },
    CapabilityVariant {
        id: ProviderId::Vast.as_str(),
        aliases: ProviderId::Vast.aliases(),
        provider: Some(ProviderId::Vast),
        implementation: "providers::vast",
        summary: "Publish and manage a Vast.ai host; not a renter provisioner.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Compute(ComputeAdapter::VastHost),
        config: &[],
    },
];

const STORAGE: &[CapabilityVariant] = &[
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
        aliases: &["stado-object"],
        provider: Some(ProviderId::Stado),
        implementation: "queue::stado_object::StadoObjectBackend",
        summary: "Shared provider-neutral queue through the authenticated Stado object API.",
        configurable: true,
        constructible: true,
        adapter: RuntimeAdapter::Storage(StorageAdapter::StadoObject),
        config: STADO_OBJECT_STORAGE_CONFIG,
    },
];

const EXECUTION: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Local.as_str(),
        aliases: ProviderId::Local.aliases(),
        provider: Some(ProviderId::Local),
        implementation: "providers::local::agent",
        summary: "Long-lived workstation or server agent.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Execution(ExecutionAdapter::Local),
        config: &[],
    },
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "providers::local::agent + monitor::reap + providers::gcp",
        summary: "Ephemeral agent running inside a GCE VM.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Execution(ExecutionAdapter::Gcp),
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Azure.as_str(),
        aliases: ProviderId::Azure.aliases(),
        provider: Some(ProviderId::Azure),
        implementation: "providers::local::agent + monitor::reap + providers::azure",
        summary: "Ephemeral agent running inside an Azure VM.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Execution(ExecutionAdapter::Azure),
        config: AZURE_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Aws.as_str(),
        aliases: ProviderId::Aws.aliases(),
        provider: Some(ProviderId::Aws),
        implementation: "providers::local::agent + monitor::reap + providers::aws",
        summary: "Ephemeral agent running inside an EC2 VM.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Execution(ExecutionAdapter::Aws),
        config: AWS_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Box.as_str(),
        aliases: ProviderId::Box.aliases(),
        provider: Some(ProviderId::Box),
        implementation: "providers::local::agent + providers::box",
        summary: "Agent bootstrapped on an externally managed box.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Execution(ExecutionAdapter::Box),
        config: &[],
    },
    CapabilityVariant {
        id: ProviderId::Vast.as_str(),
        aliases: ProviderId::Vast.aliases(),
        provider: Some(ProviderId::Vast),
        implementation: "providers::local::agent + providers::vast",
        summary: "Agent on a Vast.ai-listed host.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Execution(ExecutionAdapter::Vast),
        config: &[],
    },
];

const SCHEDULING: &[CapabilityVariant] = &[CapabilityVariant {
    id: "central-makespan",
    aliases: &[],
    provider: Some(ProviderId::Stado),
    implementation: "scheduler::scheduler + scheduler::makespan",
    summary: "Provider-neutral queue assignment and makespan minimization.",
    configurable: false,
    constructible: false,
    adapter: RuntimeAdapter::None,
    config: &[],
}];

const INVENTORY: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "providers::gcp::inventory",
        summary: "Enumerate GCP compute, storage, IAM, network, and managed-service assets.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Inventory(InventoryAdapter::Gcp),
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Azure.as_str(),
        aliases: ProviderId::Azure.aliases(),
        provider: Some(ProviderId::Azure),
        implementation: "cli::resources::inventory",
        summary:
            "Enumerate Stado-owned Azure agent VMs; broader ARM asset inventory is not implemented.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Inventory(InventoryAdapter::Azure),
        config: AZURE_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Aws.as_str(),
        aliases: ProviderId::Aws.aliases(),
        provider: Some(ProviderId::Aws),
        implementation: "cli::resources::inventory",
        summary:
            "Enumerate Stado-owned EC2 agent VMs; broader AWS asset inventory is not implemented.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Inventory(InventoryAdapter::Aws),
        config: AWS_COMPUTE_CONFIG,
    },
];

const QUOTA: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "scheduler::quota",
        summary: "Live GCP accelerator quota with configured reservations.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Quota(QuotaAdapter::Gcp),
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Azure.as_str(),
        aliases: ProviderId::Azure.aliases(),
        provider: Some(ProviderId::Azure),
        implementation: "scheduler::quota",
        summary: "Live Azure VM-family quota with configured reservations.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Quota(QuotaAdapter::Azure),
        config: AZURE_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: "storage-overlay",
        aliases: &[],
        provider: Some(ProviderId::Stado),
        implementation: "config/quotas.json",
        summary: "Provider-neutral static quota and reservation overlay.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Quota(QuotaAdapter::StorageOverlay),
        config: &[],
    },
];

const BILLING: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "monitor::billing",
        summary: "GCP credits, burn, budgets and billing-health snapshot.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Billing(BillingAdapter::Gcp),
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Azure.as_str(),
        aliases: ProviderId::Azure.aliases(),
        provider: Some(ProviderId::Azure),
        implementation: "monitor::billing",
        summary: "Azure balance, usage and billing-health snapshot.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Billing(BillingAdapter::Azure),
        config: AZURE_COMPUTE_CONFIG,
    },
];

const ARTIFACTS: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "generic-v1",
        aliases: &[],
        provider: Some(ProviderId::Stado),
        implementation: "artifacts::registry + artifacts::validation",
        summary: "Generic manifest registration and validation fallback.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "activation-dataset",
        aliases: &[],
        provider: Some(ProviderId::Huggingface),
        implementation: "artifacts::adapters::ActivationDatasetAdapter",
        summary: "Type-specific Hugging Face activation-dataset verification.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
];

const AUTHENTICATION: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "object-token",
        aliases: &[],
        provider: Some(ProviderId::Skarbiec),
        implementation: "dashboard::authorize_object",
        summary: "Namespace and key-prefix scoped product bearers resolved from mapped <namespace>-object-api/token items.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "release-publisher-token",
        aliases: &[],
        provider: Some(ProviderId::Skarbiec),
        implementation: "dashboard::authorize_release",
        summary: "Product-prefix scoped immutable release publisher bearers resolved from mapped <product>-release-publisher/token items.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "machine-token",
        aliases: &[],
        provider: Some(ProviderId::Skarbiec),
        implementation: "dashboard::authorized",
        summary: "Machine submit/status/cancel bearer stored as stado-machine-api/token.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "service-token",
        aliases: &[],
        provider: Some(ProviderId::Skarbiec),
        implementation: "dashboard::authorize_service",
        summary: "Exact service/action bearer resolved from service_api.deployers.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "host-health-token",
        aliases: &[],
        provider: Some(ProviderId::Skarbiec),
        implementation: "dashboard::authorized + cli::host::publish_beacon",
        summary: "Route-scoped host beacon publisher bearer resolved from stado-host-health-api/token.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "supabase-rls",
        aliases: &[],
        provider: Some(ProviderId::Supabase),
        implementation: "dashboard::authorized",
        summary: "User JWT authorization through the stado_can_access RLS RPC.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
];

const SECRETS: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Skarbiec.as_str(),
        aliases: ProviderId::Skarbiec.aliases(),
        provider: Some(ProviderId::Skarbiec),
        implementation: "skarbiec::Client",
        summary: "Scoped application and workload-secret retrieval.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "gcp-adc",
        aliases: &[],
        provider: Some(ProviderId::Gcp),
        implementation: "skarbiec::gcp_provider",
        summary: "GCP Application Default Credentials and workload identity.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: "aws-credential-chain",
        aliases: &[],
        provider: Some(ProviderId::Aws),
        implementation: "providers::aws::sdk_config",
        summary: "AWS credential chain, IMDS and scoped Skarbiec fallback.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: AWS_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: "azure-managed-identity",
        aliases: &[],
        provider: Some(ProviderId::Azure),
        implementation: "azure_token",
        summary: "Azure managed identity and encrypted operator session.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: AZURE_COMPUTE_CONFIG,
    },
];

const ALERTS: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Slack.as_str(),
        aliases: ProviderId::Slack.aliases(),
        provider: Some(ProviderId::Slack),
        implementation: "monitor::alerts",
        summary: "Slack incoming-webhook delivery.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: ProviderId::Telegram.as_str(),
        aliases: ProviderId::Telegram.aliases(),
        provider: Some(ProviderId::Telegram),
        implementation: "monitor::alerts",
        summary: "Telegram Bot API delivery.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: ProviderId::Sendgrid.as_str(),
        aliases: ProviderId::Sendgrid.aliases(),
        provider: Some(ProviderId::Sendgrid),
        implementation: "monitor::alerts",
        summary: "SendGrid email delivery.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: ProviderId::Most.as_str(),
        aliases: ProviderId::Most.aliases(),
        provider: Some(ProviderId::Most),
        implementation: "monitor::alerts",
        summary: "Twilio SMS delivery through the Most integration provider.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "gcp-pubsub",
        aliases: &[],
        provider: Some(ProviderId::Gcp),
        implementation: "monitor::alerts",
        summary: "GCP Pub/Sub alert publication.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: GCP_COMPUTE_CONFIG,
    },
];

const DEPLOYMENT: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "launchd",
        aliases: &[],
        provider: Some(ProviderId::Macos),
        implementation: "deploy::service",
        summary: "macOS launchd service installation and lifecycle.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "systemd",
        aliases: &[],
        provider: Some(ProviderId::Linux),
        implementation: "deploy::service",
        summary: "Linux systemd service installation and lifecycle.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "cloud-function",
        aliases: &[],
        provider: Some(ProviderId::Gcp),
        implementation: "control_plane::cloud",
        summary: "Interval-driven cloud control-plane tick.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: "vm-bootstrap",
        aliases: &[],
        provider: Some(ProviderId::MultiCloud),
        implementation: "deploy::bootstrap + data/templates",
        summary: "GCP, AWS and Azure agent VM bootstrap templates.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "https-release",
        aliases: &[],
        provider: Some(ProviderId::ProviderNeutral),
        implementation: "self_update",
        summary: "Signed binary release channel served by an HTTPS origin.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
];

const HOST_TARGET: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "targets::ComputeTarget",
        summary: "GCP dispatcher target recorded in the host registry.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Local.as_str(),
        aliases: ProviderId::Local.aliases(),
        provider: Some(ProviderId::Local),
        implementation: "targets::ComputeTarget",
        summary: "Physical or SSH-reachable host recorded in the registry.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: ProviderId::Vast.as_str(),
        aliases: ProviderId::Vast.aliases(),
        provider: Some(ProviderId::Vast),
        implementation: "targets::ComputeTarget",
        summary: "Vast.ai dispatcher pool recorded in the registry.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
];

const DEPENDENCY: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "cli::blast_radius",
        summary: "Inspect GCP-owned storage and release dependencies.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Dependency(DependencyAdapter::Gcp),
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Azure.as_str(),
        aliases: ProviderId::Azure.aliases(),
        provider: Some(ProviderId::Azure),
        implementation: "cli::blast_radius",
        summary: "Inspect Azure-owned storage and release dependencies.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Dependency(DependencyAdapter::Azure),
        config: AZURE_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Aws.as_str(),
        aliases: ProviderId::Aws.aliases(),
        provider: Some(ProviderId::Aws),
        implementation: "cli::blast_radius",
        summary: "Inspect AWS-owned storage and release dependencies.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Dependency(DependencyAdapter::Aws),
        config: AWS_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Local.as_str(),
        aliases: ProviderId::Local.aliases(),
        provider: Some(ProviderId::Local),
        implementation: "cli::blast_radius",
        summary: "Inspect device-local storage and release dependencies.",
        configurable: true,
        constructible: false,
        adapter: RuntimeAdapter::Dependency(DependencyAdapter::Local),
        config: LOCAL_STORAGE_CONFIG,
    },
];

pub static REGISTRY: &[Capability] = &[
    Capability {
        kind: RuntimeFacet::Compute,
        selection: SelectionMode::OrderedMany,
        summary: "Machine provisioning and lifecycle management.",
        variants: COMPUTE,
    },
    Capability {
        kind: RuntimeFacet::Storage,
        selection: SelectionMode::Single,
        summary: "Queue state, control data and provider-neutral product objects.",
        variants: STORAGE,
    },
    Capability {
        kind: RuntimeFacet::Execution,
        selection: SelectionMode::Automatic,
        summary: "Job execution inside long-lived or ephemeral agents.",
        variants: EXECUTION,
    },
    Capability {
        kind: RuntimeFacet::Scheduling,
        selection: SelectionMode::Internal,
        summary: "Provider-neutral assignment, dispatch and recurring schedules.",
        variants: SCHEDULING,
    },
    Capability {
        kind: RuntimeFacet::Inventory,
        selection: SelectionMode::ConcurrentMany,
        summary: "Provider-owned asset discovery.",
        variants: INVENTORY,
    },
    Capability {
        kind: RuntimeFacet::Quota,
        selection: SelectionMode::Automatic,
        summary: "Cloud quota discovery and configured capacity reservations.",
        variants: QUOTA,
    },
    Capability {
        kind: RuntimeFacet::Billing,
        selection: SelectionMode::ConcurrentMany,
        summary: "Cloud balances, budgets, burn and billing health.",
        variants: BILLING,
    },
    Capability {
        kind: RuntimeFacet::Artifacts,
        selection: SelectionMode::Automatic,
        summary: "Artifact manifests, registry and type-specific verification.",
        variants: ARTIFACTS,
    },
    Capability {
        kind: RuntimeFacet::Authentication,
        selection: SelectionMode::Automatic,
        summary: "Dashboard and object-API request authorization.",
        variants: AUTHENTICATION,
    },
    Capability {
        kind: RuntimeFacet::Secrets,
        selection: SelectionMode::Automatic,
        summary: "Application secrets and cloud workload identity.",
        variants: SECRETS,
    },
    Capability {
        kind: RuntimeFacet::Alerts,
        selection: SelectionMode::ConcurrentMany,
        summary: "Fault-isolated operator alert delivery.",
        variants: ALERTS,
    },
    Capability {
        kind: RuntimeFacet::Deployment,
        selection: SelectionMode::Automatic,
        summary: "Service installation, VM bootstrap and binary releases.",
        variants: DEPLOYMENT,
    },
    Capability {
        kind: RuntimeFacet::HostTarget,
        selection: SelectionMode::OrderedMany,
        summary: "Host registry target kinds.",
        variants: HOST_TARGET,
    },
    Capability {
        kind: RuntimeFacet::Dependency,
        selection: SelectionMode::Single,
        summary: "Blast-radius dependency ownership and inspection.",
        variants: DEPENDENCY,
    },
];

pub fn all() -> &'static [Capability] {
    REGISTRY
}

pub fn get(id: &str) -> Option<&'static Capability> {
    REGISTRY
        .iter()
        .find(|capability| capability.kind.as_str() == id)
}

pub fn variant(kind: RuntimeFacet, name: &str) -> Option<&'static CapabilityVariant> {
    let capability = REGISTRY.iter().find(|entry| entry.kind == kind)?;
    capability
        .variants
        .iter()
        .find(|entry| entry.id == name || entry.aliases.contains(&name))
}

pub fn storage_adapter(name: &str) -> Option<StorageAdapter> {
    match variant(RuntimeFacet::Storage, name).map(|variant| variant.adapter) {
        Some(RuntimeAdapter::Storage(adapter)) => Some(adapter),
        _ => None,
    }
}

pub fn execution_adapter(name: &str) -> Option<ExecutionAdapter> {
    match variant(RuntimeFacet::Execution, name).map(|variant| variant.adapter) {
        Some(RuntimeAdapter::Execution(adapter)) => Some(adapter),
        _ => None,
    }
}

pub fn configurable_variant(kind: RuntimeFacet, name: &str) -> Option<&'static CapabilityVariant> {
    variant(kind, name).filter(|entry| entry.configurable)
}

pub fn constructible_variant(kind: RuntimeFacet, name: &str) -> Option<&'static CapabilityVariant> {
    variant(kind, name).filter(|entry| {
        if !entry.constructible {
            return false;
        }
        entry.provider.is_none_or(|provider| {
            matches!(
                capability_support(kind.product_capability(), provider),
                CapabilitySupport::Implemented | CapabilitySupport::Partial
            )
        })
    })
}

pub fn configurable_ids(kind: RuntimeFacet) -> impl Iterator<Item = &'static str> {
    REGISTRY
        .iter()
        .find(|entry| entry.kind == kind)
        .into_iter()
        .flat_map(|entry| entry.variants)
        .filter(|variant| variant.configurable)
        .map(|variant| variant.id)
}

pub fn config_fields(kind: RuntimeFacet) -> impl Iterator<Item = &'static ConfigField> {
    REGISTRY
        .iter()
        .find(|entry| entry.kind == kind)
        .into_iter()
        .flat_map(|entry| entry.variants)
        .flat_map(|variant| variant.config)
}

pub fn config_field(
    kind: RuntimeFacet,
    variant_name: &str,
    key: &str,
) -> Option<&'static ConfigField> {
    variant(kind, variant_name)?
        .config
        .iter()
        .find(|field| field.key == key)
}

pub fn config_envs(kind: RuntimeFacet) -> impl Iterator<Item = &'static str> {
    config_fields(kind)
        .flat_map(|field| [Some(field.env), field.backup_env])
        .flatten()
}

pub fn backup_config_envs(kind: RuntimeFacet) -> impl Iterator<Item = &'static str> {
    config_fields(kind).filter_map(|field| field.backup_env)
}

pub fn config_env(kind: RuntimeFacet, variant_name: &str, key: &str) -> Option<&'static str> {
    config_field(kind, variant_name, key).map(|field| field.env)
}

pub fn same_variant(kind: RuntimeFacet, left: &str, right: &str) -> bool {
    match (variant(kind, left), variant(kind, right)) {
        (Some(left), Some(right)) => left.id == right.id,
        _ => left == right,
    }
}

pub fn provider_ids(kind: RuntimeFacet) -> Vec<ProviderId> {
    let mut providers = Vec::new();
    if let Some(capability) = REGISTRY.iter().find(|entry| entry.kind == kind) {
        for provider in capability
            .variants
            .iter()
            .filter_map(|variant| variant.provider)
        {
            if !providers.contains(&provider) {
                providers.push(provider);
            }
        }
    }
    providers
}

pub fn canonical_id(kind: RuntimeFacet, name: &str) -> Option<&'static str> {
    variant(kind, name).map(|variant| variant.id)
}

fn config_binding_incomplete(field: &ConfigField) -> bool {
    field.key.is_empty() || field.env.is_empty() || field.path.is_empty()
}

fn backup_binding_incomplete(field: &ConfigField) -> bool {
    field.backup_path.is_some() != field.backup_env.is_some()
        || (field.backup_required && field.backup_path.is_none())
}

fn validate_product_catalog(problems: &mut Vec<String>) {
    let mut ids = BTreeSet::new();
    for capability in CAPABILITIES {
        if !ids.insert(capability.id) {
            problems.push(format!(
                "duplicate product capability {}",
                capability.id.as_str()
            ));
        }
        if capability.summary.trim().is_empty() {
            problems.push(format!(
                "product capability {} has no user-facing summary",
                capability.id.as_str()
            ));
        }
        if capability.providers.is_empty() {
            problems.push(format!(
                "product capability {} has no provider support entries",
                capability.id.as_str()
            ));
        }
        let mut providers = BTreeSet::new();
        for support in capability.providers {
            if !providers.insert(support.provider) {
                problems.push(format!(
                    "product capability {} declares provider {} more than once",
                    capability.id.as_str(),
                    support.provider
                ));
            }
            if matches!(
                support.support,
                CapabilitySupport::Implemented | CapabilitySupport::Partial
            ) && support.implementation.trim().is_empty()
            {
                problems.push(format!(
                    "product capability {} marks {} as {} without an implementation",
                    capability.id.as_str(),
                    support.provider,
                    support.support.as_str()
                ));
            }
            if support.note.trim().is_empty() {
                problems.push(format!(
                    "product capability {} has an unexplained {} support row",
                    capability.id.as_str(),
                    support.provider
                ));
            }
            if support.support == CapabilitySupport::Unsupported {
                problems.push(format!(
                    "product capability {} explicitly lists unsupported provider {}; omit the row instead",
                    capability.id.as_str(),
                    support.provider
                ));
            }
        }
    }
}

/// Static integrity audit for the catalog itself. This is intentionally
/// allocation-light and runs only on the operator-facing discovery path.
pub fn validate_catalog() -> Vec<String> {
    let mut problems = Vec::new();
    validate_product_catalog(&mut problems);
    for field in CONTROL_CONFIG {
        if config_binding_incomplete(field) || backup_binding_incomplete(field) {
            problems.push(format!(
                "control field {} has an incomplete configuration binding",
                field.key
            ));
        }
    }
    let mut kinds = BTreeSet::new();
    for capability in REGISTRY {
        if !kinds.insert(capability.kind) {
            problems.push(format!(
                "duplicate capability kind {}",
                capability.kind.as_str()
            ));
        }
        let mut names = BTreeSet::new();
        for variant in capability.variants {
            if !names.insert(variant.id) {
                problems.push(format!(
                    "{} has duplicate variant {:?}",
                    capability.kind.as_str(),
                    variant.id
                ));
            }
            for alias in variant.aliases {
                if !names.insert(alias) {
                    problems.push(format!(
                        "{} has colliding alias {:?}",
                        capability.kind.as_str(),
                        alias
                    ));
                }
            }
            if variant
                .provider
                .is_some_and(|provider| !PROVIDERS.contains(&provider))
            {
                problems.push(format!(
                    "{}.{} refers to an unregistered provider",
                    capability.kind.as_str(),
                    variant.id
                ));
            }
            if variant.constructible
                && variant.provider.is_some_and(|provider| {
                    !matches!(
                        capability_support(capability.kind.product_capability(), provider),
                        CapabilitySupport::Implemented | CapabilitySupport::Partial
                    )
                })
            {
                problems.push(format!(
                    "{}.{} is constructible but its provider does not implement product capability {}",
                    capability.kind.as_str(),
                    variant.id,
                    capability.kind.product_capability()
                ));
            }
            if let Some(provider) = variant.adapter.provider() {
                if variant.provider != Some(provider) {
                    problems.push(format!(
                        "{}.{} adapter belongs to {}, but the variant declares {:?}",
                        capability.kind.as_str(),
                        variant.id,
                        provider,
                        variant.provider
                    ));
                }
            }
            let adapter_invalid = match variant.adapter {
                RuntimeAdapter::None => {
                    runtime_backed_kind(capability.kind) && variant.constructible
                }
                adapter => !adapter_matches_kind(capability.kind, adapter),
            };
            if adapter_invalid {
                problems.push(format!(
                    "{}.{} has an incompatible runtime adapter",
                    capability.kind.as_str(),
                    variant.id
                ));
            }
            let mut field_keys = BTreeSet::new();
            for field in variant.config {
                if !field_keys.insert(field.key) {
                    problems.push(format!(
                        "{}.{} has duplicate configuration key {}",
                        capability.kind.as_str(),
                        variant.id,
                        field.key
                    ));
                }
                if config_binding_incomplete(field) {
                    problems.push(format!(
                        "{}.{} has an incomplete configuration binding",
                        capability.kind.as_str(),
                        variant.id
                    ));
                }
                if backup_binding_incomplete(field) {
                    problems.push(format!(
                        "{}.{} field {} has an incomplete backup path/env binding",
                        capability.kind.as_str(),
                        variant.id,
                        field.key
                    ));
                }
            }
        }
    }

    let mut provider_names = BTreeSet::new();
    for provider in PROVIDERS {
        if !provider_names.insert(provider.as_str()) {
            problems.push(format!("duplicate provider id {}", provider.as_str()));
        }
        for alias in provider.aliases() {
            if !provider_names.insert(alias) {
                problems.push(format!("colliding provider alias {alias:?}"));
            }
        }
    }
    problems
}

fn runtime_backed_kind(kind: RuntimeFacet) -> bool {
    matches!(
        kind,
        RuntimeFacet::Compute
            | RuntimeFacet::Storage
            | RuntimeFacet::Execution
            | RuntimeFacet::Inventory
            | RuntimeFacet::Quota
            | RuntimeFacet::Billing
            | RuntimeFacet::Dependency
    )
}

fn adapter_matches_kind(kind: RuntimeFacet, adapter: RuntimeAdapter) -> bool {
    matches!(
        (kind, adapter),
        (RuntimeFacet::Compute, RuntimeAdapter::Compute(_))
            | (RuntimeFacet::Storage, RuntimeAdapter::Storage(_))
            | (RuntimeFacet::Execution, RuntimeAdapter::Execution(_))
            | (RuntimeFacet::Inventory, RuntimeAdapter::Inventory(_))
            | (RuntimeFacet::Quota, RuntimeAdapter::Quota(_))
            | (RuntimeFacet::Billing, RuntimeAdapter::Billing(_))
            | (RuntimeFacet::Dependency, RuntimeAdapter::Dependency(_))
    )
}
