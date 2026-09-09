//! The one declaration of capability ids, summaries and provider support.

use std::fmt;
use std::str::FromStr;

use serde::Serialize;

use super::macros::define_capabilities;
use super::providers::ProviderId;
use super::support::{CapabilitySupport, ProductCapability, ProviderCapability};

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
            ProviderId::Resend => (Implemented, "monitor::alerts", "Resend email delivery"),
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
    MobileAppCapture => {
        id: "mobile-app-capture",
        summary: "Drive and capture installed iOS and Android applications on a declared host.",
        providers: [
            ProviderId::Macos => (Implemented, "deploy::mobile_runtime", "Appium with the XCUITest driver for iOS and UiAutomator2 for Android, declared per host under targets[].mobile_runtime and verified at its declared absolute paths"),
            ProviderId::Local => (Partial, "deploy::mobile_runtime", "A registry host carries the runtime only where it declares one; iOS additionally requires Xcode, which no fleet host installs on demand"),
            ProviderId::Linux => (Planned, "", "Android would work through platform-tools; iOS cannot be driven from Linux at all"),
        ]
    },
    IdentityAccess => {
        id: "identity-access",
        summary: "Authenticate workloads and authorize provider operations.",
        providers: [
            ProviderId::Skarbiec => (Implemented, "dashboard authorization + skarbiec::Client", "Scoped bearer resolution for Stado APIs"),
            ProviderId::Gcp => (Implemented, "skarbiec::gcp_provider", "Application Default Credentials and workload identity"),
            ProviderId::Azure => (Implemented, "azure_token", "Managed identity and operator token chain"),
            ProviderId::Aws => (Implemented, "providers::aws::sdk_config", "AWS credential chain, IMDS, and the scoped Skarbiec identity"),
            ProviderId::Local => (Partial, "deploy::host_channel", "Local account and SSH identity"),
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
            ProviderId::Local => (Implemented, "queue::capacity + providers::local", "Live CPU, RAM, disk, and accelerator measurements"),
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
