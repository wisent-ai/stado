//! Central catalog of Stado capability families and their built-in variants.
//!
//! This is the source of truth for operator-facing discovery and for validating
//! configurable compute and storage names. Runtime factories still own object
//! construction, but they resolve names through this catalog first, so aliases
//! and accepted configuration cannot drift independently.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityKind {
    Compute,
    Storage,
    Execution,
    Scheduling,
    Quota,
    Billing,
    Artifacts,
    Authentication,
    Secrets,
    Alerts,
    Deployment,
}

impl CapabilityKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compute => "compute",
            Self::Storage => "storage",
            Self::Execution => "execution",
            Self::Scheduling => "scheduling",
            Self::Quota => "quota",
            Self::Billing => "billing",
            Self::Artifacts => "artifacts",
            Self::Authentication => "authentication",
            Self::Secrets => "secrets",
            Self::Alerts => "alerts",
            Self::Deployment => "deployment",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CapabilityVariant {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub provider: Option<&'static str>,
    pub implementation: &'static str,
    pub summary: &'static str,
    /// Accepted in the capability's public configuration surface.
    pub configurable: bool,
    /// A generic runtime factory can construct this variant.
    pub constructible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Capability {
    pub kind: CapabilityKind,
    pub selection: SelectionMode,
    pub summary: &'static str,
    pub variants: &'static [CapabilityVariant],
}

const COMPUTE: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "gcp",
        aliases: &[],
        provider: Some("gcp"),
        implementation: "providers::gcp::GcpProvider",
        summary: "Provision and reap Google Compute Engine instances.",
        configurable: true,
        constructible: true,
    },
    CapabilityVariant {
        id: "azure",
        aliases: &[],
        provider: Some("azure"),
        implementation: "providers::azure::AzureProvider",
        summary: "Provision and reap Azure virtual machines.",
        configurable: true,
        constructible: true,
    },
    CapabilityVariant {
        id: "aws",
        aliases: &[],
        provider: Some("aws"),
        implementation: "providers::aws::AwsProvider",
        summary: "Provision and reap Amazon EC2 instances.",
        configurable: true,
        constructible: true,
    },
    CapabilityVariant {
        id: "box",
        aliases: &["box-ascii"],
        provider: Some("box"),
        implementation: "providers::box::BoxProvider",
        summary: "Lease externally managed fixed-shape boxes.",
        configurable: true,
        constructible: true,
    },
    CapabilityVariant {
        id: "local",
        aliases: &[],
        provider: Some("local"),
        implementation: "providers::local",
        summary: "Execute on an existing local host without VM provisioning.",
        configurable: true,
        constructible: false,
    },
    CapabilityVariant {
        id: "vast",
        aliases: &[],
        provider: Some("vast"),
        implementation: "providers::vast",
        summary: "Publish and manage a Vast.ai host; not a renter provisioner.",
        configurable: false,
        constructible: false,
    },
];

const STORAGE: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "gcs",
        aliases: &[],
        provider: Some("gcp"),
        implementation: "queue::gcs::GcsBackend",
        summary: "Shared Google Cloud Storage queue and object store.",
        configurable: true,
        constructible: true,
    },
    CapabilityVariant {
        id: "azure",
        aliases: &[],
        provider: Some("azure"),
        implementation: "queue::azure_blob::AzureBlobBackend",
        summary: "Shared Azure Blob queue and object store.",
        configurable: true,
        constructible: true,
    },
    CapabilityVariant {
        id: "s3",
        aliases: &[],
        provider: Some("aws"),
        implementation: "queue::s3::S3Backend",
        summary: "Shared Amazon S3 queue and object store.",
        configurable: true,
        constructible: true,
    },
    CapabilityVariant {
        id: "local",
        aliases: &[],
        provider: Some("local"),
        implementation: "queue::local_file::LocalBackend",
        summary: "Device-local filesystem store for one-host operation.",
        configurable: true,
        constructible: true,
    },
];

const EXECUTION: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "local",
        aliases: &[],
        provider: Some("local"),
        implementation: "providers::local::agent",
        summary: "Long-lived workstation or server agent.",
        configurable: true,
        constructible: false,
    },
    CapabilityVariant {
        id: "gcp",
        aliases: &[],
        provider: Some("gcp"),
        implementation: "providers::local::agent + gcp_self",
        summary: "Ephemeral agent running inside a GCE VM.",
        configurable: true,
        constructible: false,
    },
    CapabilityVariant {
        id: "azure",
        aliases: &[],
        provider: Some("azure"),
        implementation: "providers::local::agent + azure_self",
        summary: "Ephemeral agent running inside an Azure VM.",
        configurable: true,
        constructible: false,
    },
    CapabilityVariant {
        id: "aws",
        aliases: &[],
        provider: Some("aws"),
        implementation: "providers::local::agent",
        summary: "Ephemeral agent running inside an EC2 VM.",
        configurable: true,
        constructible: false,
    },
    CapabilityVariant {
        id: "vast",
        aliases: &[],
        provider: Some("vast"),
        implementation: "providers::local::agent + providers::vast",
        summary: "Agent on a Vast.ai-listed host.",
        configurable: true,
        constructible: false,
    },
];

const SCHEDULING: &[CapabilityVariant] = &[CapabilityVariant {
    id: "central-makespan",
    aliases: &[],
    provider: Some("stado"),
    implementation: "scheduler::scheduler + scheduler::makespan",
    summary: "Provider-neutral queue assignment and makespan minimization.",
    configurable: false,
    constructible: false,
}];

const QUOTA: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "gcp",
        aliases: &[],
        provider: Some("gcp"),
        implementation: "scheduler::quota",
        summary: "Live GCP accelerator quota with configured reservations.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "azure",
        aliases: &[],
        provider: Some("azure"),
        implementation: "scheduler::quota",
        summary: "Live Azure VM-family quota with configured reservations.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "storage-overlay",
        aliases: &[],
        provider: Some("stado"),
        implementation: "config/quotas.json",
        summary: "Provider-neutral static quota and reservation overlay.",
        configurable: false,
        constructible: false,
    },
];

const BILLING: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "gcp",
        aliases: &[],
        provider: Some("gcp"),
        implementation: "monitor::billing",
        summary: "GCP credits, burn, budgets and billing-health snapshot.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "azure",
        aliases: &[],
        provider: Some("azure"),
        implementation: "monitor::billing",
        summary: "Azure balance, usage and billing-health snapshot.",
        configurable: false,
        constructible: false,
    },
];

const ARTIFACTS: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "generic-v1",
        aliases: &[],
        provider: Some("stado"),
        implementation: "artifacts::registry + artifacts::validation",
        summary: "Generic manifest registration and validation fallback.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "activation-dataset",
        aliases: &[],
        provider: Some("huggingface"),
        implementation: "artifacts::adapters::ActivationDatasetAdapter",
        summary: "Type-specific Hugging Face activation-dataset verification.",
        configurable: false,
        constructible: false,
    },
];

const AUTHENTICATION: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "object-token",
        aliases: &[],
        provider: Some("skarbiec"),
        implementation: "dashboard::authorized",
        summary: "Object bearer token stored as stado-object-api/token.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "machine-token",
        aliases: &[],
        provider: Some("skarbiec"),
        implementation: "dashboard::authorized",
        summary: "Machine submit/status/cancel bearer stored as stado-machine-api/token.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "service-token",
        aliases: &[],
        provider: Some("skarbiec"),
        implementation: "dashboard::authorized",
        summary: "Managed-service status/restart bearer stored as stado-service-api/token.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "supabase-rls",
        aliases: &[],
        provider: Some("supabase"),
        implementation: "dashboard::authorized",
        summary: "User JWT authorization through the stado_can_access RLS RPC.",
        configurable: false,
        constructible: false,
    },
];

const SECRETS: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "skarbiec",
        aliases: &[],
        provider: Some("skarbiec"),
        implementation: "skarbiec::Client",
        summary: "Scoped application and workload-secret retrieval.",
        configurable: true,
        constructible: false,
    },
    CapabilityVariant {
        id: "gcp-adc",
        aliases: &[],
        provider: Some("gcp"),
        implementation: "skarbiec::gcp_provider",
        summary: "GCP Application Default Credentials and workload identity.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "aws-credential-chain",
        aliases: &[],
        provider: Some("aws"),
        implementation: "providers::aws::sdk_config",
        summary: "AWS credential chain, IMDS and scoped Skarbiec fallback.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "azure-managed-identity",
        aliases: &[],
        provider: Some("azure"),
        implementation: "azure_token",
        summary: "Azure managed identity and encrypted operator session.",
        configurable: false,
        constructible: false,
    },
];

const ALERTS: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "slack",
        aliases: &[],
        provider: Some("slack"),
        implementation: "monitor::alerts",
        summary: "Slack incoming-webhook delivery.",
        configurable: true,
        constructible: false,
    },
    CapabilityVariant {
        id: "telegram",
        aliases: &[],
        provider: Some("telegram"),
        implementation: "monitor::alerts",
        summary: "Telegram Bot API delivery.",
        configurable: true,
        constructible: false,
    },
    CapabilityVariant {
        id: "sendgrid",
        aliases: &[],
        provider: Some("sendgrid"),
        implementation: "monitor::alerts",
        summary: "SendGrid email delivery.",
        configurable: true,
        constructible: false,
    },
    CapabilityVariant {
        id: "gcp-pubsub",
        aliases: &[],
        provider: Some("gcp"),
        implementation: "monitor::alerts",
        summary: "GCP Pub/Sub alert publication.",
        configurable: true,
        constructible: false,
    },
];

const DEPLOYMENT: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "launchd",
        aliases: &[],
        provider: Some("macos"),
        implementation: "deploy::service",
        summary: "macOS launchd service installation and lifecycle.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "systemd",
        aliases: &[],
        provider: Some("linux"),
        implementation: "deploy::service",
        summary: "Linux systemd service installation and lifecycle.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "cloud-function",
        aliases: &[],
        provider: Some("gcp"),
        implementation: "control_plane::cloud",
        summary: "Interval-driven cloud control-plane tick.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "vm-bootstrap",
        aliases: &[],
        provider: Some("multi-cloud"),
        implementation: "deploy::bootstrap + data/templates",
        summary: "GCP, AWS and Azure agent VM bootstrap templates.",
        configurable: false,
        constructible: false,
    },
    CapabilityVariant {
        id: "https-release",
        aliases: &[],
        provider: Some("provider-neutral"),
        implementation: "self_update",
        summary: "Signed binary release channel served by an HTTPS origin.",
        configurable: true,
        constructible: false,
    },
];

pub static REGISTRY: &[Capability] = &[
    Capability {
        kind: CapabilityKind::Compute,
        selection: SelectionMode::OrderedMany,
        summary: "Machine provisioning and lifecycle management.",
        variants: COMPUTE,
    },
    Capability {
        kind: CapabilityKind::Storage,
        selection: SelectionMode::Single,
        summary: "Queue state, control data and provider-neutral product objects.",
        variants: STORAGE,
    },
    Capability {
        kind: CapabilityKind::Execution,
        selection: SelectionMode::Automatic,
        summary: "Job execution inside long-lived or ephemeral agents.",
        variants: EXECUTION,
    },
    Capability {
        kind: CapabilityKind::Scheduling,
        selection: SelectionMode::Internal,
        summary: "Provider-neutral assignment, dispatch and recurring schedules.",
        variants: SCHEDULING,
    },
    Capability {
        kind: CapabilityKind::Quota,
        selection: SelectionMode::Automatic,
        summary: "Cloud quota discovery and configured capacity reservations.",
        variants: QUOTA,
    },
    Capability {
        kind: CapabilityKind::Billing,
        selection: SelectionMode::ConcurrentMany,
        summary: "Cloud balances, budgets, burn and billing health.",
        variants: BILLING,
    },
    Capability {
        kind: CapabilityKind::Artifacts,
        selection: SelectionMode::Automatic,
        summary: "Artifact manifests, registry and type-specific verification.",
        variants: ARTIFACTS,
    },
    Capability {
        kind: CapabilityKind::Authentication,
        selection: SelectionMode::Automatic,
        summary: "Dashboard and object-API request authorization.",
        variants: AUTHENTICATION,
    },
    Capability {
        kind: CapabilityKind::Secrets,
        selection: SelectionMode::Automatic,
        summary: "Application secrets and cloud workload identity.",
        variants: SECRETS,
    },
    Capability {
        kind: CapabilityKind::Alerts,
        selection: SelectionMode::ConcurrentMany,
        summary: "Fault-isolated operator alert delivery.",
        variants: ALERTS,
    },
    Capability {
        kind: CapabilityKind::Deployment,
        selection: SelectionMode::Automatic,
        summary: "Service installation, VM bootstrap and binary releases.",
        variants: DEPLOYMENT,
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

pub fn variant(kind: CapabilityKind, name: &str) -> Option<&'static CapabilityVariant> {
    let capability = REGISTRY.iter().find(|entry| entry.kind == kind)?;
    capability
        .variants
        .iter()
        .find(|entry| entry.id == name || entry.aliases.contains(&name))
}

pub fn configurable_variant(
    kind: CapabilityKind,
    name: &str,
) -> Option<&'static CapabilityVariant> {
    variant(kind, name).filter(|entry| entry.configurable)
}

pub fn constructible_variant(
    kind: CapabilityKind,
    name: &str,
) -> Option<&'static CapabilityVariant> {
    variant(kind, name).filter(|entry| entry.constructible)
}

pub fn configurable_ids(kind: CapabilityKind) -> impl Iterator<Item = &'static str> {
    REGISTRY
        .iter()
        .find(|entry| entry.kind == kind)
        .into_iter()
        .flat_map(|entry| entry.variants)
        .filter(|variant| variant.configurable)
        .map(|variant| variant.id)
}
