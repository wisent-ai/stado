//! The one declaration of capability ids, summaries and provider support.

use std::{fmt, str::FromStr};

use super::macros::{define_capabilities, provider_rows};
use super::providers::ProviderId;
use super::support::{CapabilitySupport, ProductCapability, ProviderCapability};
use serde::Serialize;

mod storage;

define_capabilities! {
    Compute => {
        id: "compute",
        summary: "Provision or attach CPU and accelerator-backed machines.",
        providers: provider_rows![
            ProviderId::Gcp => (Partial, "providers::gcp::GcpProvider", "Preview Google Compute Engine VM lifecycle; not stable without release-scoped live acceptance"),
            ProviderId::Azure => (Implemented, "providers::azure::AzureProvider", "Preview Azure Virtual Machines lifecycle; not stable without release-scoped live acceptance"),
            ProviderId::Aws => (Implemented, "providers::aws::AwsProvider", "Preview Amazon EC2 lifecycle; not stable without release-scoped live acceptance"),
            ProviderId::Boat => (Implemented, "providers::box::BoxProvider", "Boat sandboxes: create, renew, stop or delete"),
            ProviderId::Local => (Partial, "providers::local", "Attach existing hosts; no machine provisioning"),
            ProviderId::Vast => (Partial, "providers::vast", "Publish this fleet's host; renting Vast.ai machines is provider vast-rental"),
            ProviderId::Arkane => (Partial, "providers::gpu_cloud::vendors::arkane", "Arkane Cloud instances are listed, read and released; its deploy API takes no startup script, so no agent is dispatched there"),
            ProviderId::Crusoe => (Implemented, "providers::gpu_cloud::vendors::crusoe", "Crusoe Cloud VM lifecycle"),
            ProviderId::Cudo => (Implemented, "providers::gpu_cloud::vendors::cudo", "Cudo Compute VM lifecycle"),
            ProviderId::Hyperstack => (Implemented, "providers::gpu_cloud::vendors::hyperstack", "Hyperstack virtual machine lifecycle"),
            ProviderId::Lambda => (Implemented, "providers::gpu_cloud::vendors::lambda", "Lambda Cloud on-demand instance lifecycle"),
            ProviderId::Latitude => (Implemented, "providers::gpu_cloud::vendors::latitude", "Latitude.sh GPU virtual machine lifecycle"),
            ProviderId::Nebius => (Implemented, "providers::gpu_cloud::vendors::nebius", "Nebius AI Cloud Compute instance lifecycle"),
            ProviderId::Oblivus => (Implemented, "providers::gpu_cloud::vendors::oblivus", "Oblivus Cloud virtual machine lifecycle"),
            ProviderId::Oracle => (Implemented, "providers::gpu_cloud::vendors::oracle", "Oracle Cloud Infrastructure GPU instance lifecycle"),
            ProviderId::Runpod => (Implemented, "providers::gpu_cloud::vendors::runpod", "RunPod GPU pod lifecycle; the agent runs inside the pod's container"),
            ProviderId::Salad => (Implemented, "providers::gpu_cloud::vendors::salad", "SaladCloud single-replica container group lifecycle"),
            ProviderId::Scaleway => (Implemented, "providers::gpu_cloud::vendors::scaleway", "Scaleway GPU Instance lifecycle"),
            ProviderId::VastRental => (Implemented, "providers::gpu_cloud::vendors::vast_rental", "Vast.ai instance lifecycle: the cheapest verified on-demand offer for an accelerator is rented, read and destroyed"),
            ProviderId::VoltagePark => (Implemented, "providers::gpu_cloud::vendors::voltage_park", "Voltage Park on-demand virtual machine lifecycle"),
            ProviderId::Vultr => (Implemented, "providers::gpu_cloud::vendors::vultr", "Vultr Cloud GPU instance lifecycle"),
        ]
    },
    ManagedCompute => {
        id: "managed-compute",
        summary: "Operate autoscaled or provider-managed groups of machines.",
        providers: provider_rows![
            ProviderId::Gcp => (Partial, "cli::resources", "Managed instance groups and templates are inspected and selected mutations are supported"),
            ProviderId::Azure => (Planned, "", "Azure Virtual Machine Scale Sets equivalent; no Stado adapter"),
            ProviderId::Aws => (Planned, "", "Amazon EC2 Auto Scaling equivalent; no Stado adapter"),
            ProviderId::Local => (External, "targets + coordinator", "Local host pools are scheduled but not autoscaled"),
        ]
    },
    WorkloadExecution => {
        id: "workload-execution",
        summary: "Run a queued workload in a managed agent environment.",
        providers: provider_rows![
            ProviderId::Stado => (Implemented, "scheduler + agent", "Provider-neutral queue, leases, dispatch, and execution contract"),
            ProviderId::Gcp => (Partial, "providers::local::agent + providers::gcp", "Preview ephemeral agent lifecycle on an owned GCE VM"),
            ProviderId::Azure => (Implemented, "providers::local::agent + providers::azure", "Preview ephemeral agent lifecycle on an Azure VM"),
            ProviderId::Aws => (Implemented, "providers::local::agent + providers::aws", "Preview ephemeral agent lifecycle on an EC2 VM"),
            ProviderId::Boat => (Implemented, "providers::local::agent + providers::box", "Command or agent-prompt job inside a Boat sandbox"),
            ProviderId::Local => (Implemented, "providers::local::agent", "Long-lived workstation or server agent"),
            ProviderId::Vast => (Partial, "providers::local::agent + providers::vast", "Agent execution on an operator-published Vast host; agents on rented Vast.ai machines are provider vast-rental"),
            ProviderId::Crusoe => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on a Crusoe VM"),
            ProviderId::Cudo => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on a Cudo Compute VM"),
            ProviderId::Hyperstack => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on a Hyperstack VM"),
            ProviderId::Lambda => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on a Lambda Cloud instance"),
            ProviderId::Latitude => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on a Latitude.sh virtual machine"),
            ProviderId::Nebius => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on a Nebius instance"),
            ProviderId::Oblivus => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on an Oblivus VM"),
            ProviderId::Oracle => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on an OCI instance"),
            ProviderId::Runpod => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent inside a RunPod pod"),
            ProviderId::Salad => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent inside a SaladCloud container"),
            ProviderId::Scaleway => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on a Scaleway GPU Instance"),
            ProviderId::VastRental => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent inside a rented Vast.ai instance's container"),
            ProviderId::VoltagePark => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on a Voltage Park VM"),
            ProviderId::Vultr => (Implemented, "providers::local::agent + providers::gpu_cloud", "Ephemeral agent on a Vultr Cloud GPU instance"),
        ]
    },
    ObjectStorage => {
        id: "object-storage",
        summary: "Persist queue state, results, artifacts, and control objects.",
        providers: storage::OBJECT_STORAGE
    },
    BlockStorage => {
        id: "block-storage",
        summary: "Attach and manage machine boot or persistent block devices.",
        providers: provider_rows![
            ProviderId::Gcp => (Partial, "providers::gcp + cli::resources", "Boot disks and selected Compute Engine disk operations"),
            ProviderId::Azure => (Partial, "providers::azure", "VM operating-system disk provisioning"),
            ProviderId::Aws => (Partial, "providers::aws", "EC2 root-volume provisioning"),
            ProviderId::Local => (External, "host operating system", "Local disks are consumed but not provisioned by Stado"),
        ]
    },
    MachineImages => {
        id: "machine-images",
        summary: "Select immutable images used to bootstrap workload machines.",
        providers: provider_rows![
            ProviderId::Gcp => (Implemented, "providers::gcp", "Compute Engine image projects and families"),
            ProviderId::Azure => (Implemented, "providers::azure", "Azure image URNs"),
            ProviderId::Aws => (Implemented, "providers::aws", "Amazon Machine Images"),
            ProviderId::Local => (External, "host operating system", "Existing host installation is managed outside machine provisioning"),
        ]
    },
    ContainerRegistry => {
        id: "container-registry",
        summary: "Store and retrieve versioned container images.",
        providers: provider_rows![
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Artifact Registry is inventoried but not managed by a Stado adapter"),
            ProviderId::Azure => (Planned, "", "Azure Container Registry equivalent; no Stado adapter"),
            ProviderId::Aws => (Planned, "", "Amazon Elastic Container Registry equivalent; no Stado adapter"),
            ProviderId::Local => (External, "container runtime", "Local image storage is owned by the installed container runtime"),
        ]
    },
    ApplicationHosting => {
        id: "application-hosting",
        summary: "Run a continuously available service or control plane.",
        providers: provider_rows![
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Cloud Run services are inventoried; provisioning is external"),
            ProviderId::Azure => (Planned, "", "No Azure application-hosting adapter"),
            ProviderId::Aws => (Planned, "", "No AWS application-hosting adapter"),
            ProviderId::Local => (Implemented, "deploy::service", "launchd and systemd service lifecycle"),
        ]
    },
    ServerlessFunctions => {
        id: "serverless-functions",
        summary: "Execute an event-driven or interval-driven stateless function.",
        providers: provider_rows![
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Legacy Cloud Function is observable but retired from the active control plane"),
            ProviderId::Azure => (Planned, "", "Azure Functions equivalent; no Stado adapter"),
            ProviderId::Aws => (Planned, "", "AWS Lambda equivalent; no Stado adapter"),
        ]
    },
    Scheduling => {
        id: "scheduling",
        summary: "Assign queued work and trigger recurring operations.",
        providers: provider_rows![
            ProviderId::Stado => (Implemented, "scheduler + schedules", "Makespan assignment and recurring schedules"),
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Cloud Scheduler is inventoried but not the active scheduler"),
            ProviderId::Local => (Implemented, "serve --control-plane local", "Long-running local coordinator inside stado serve"),
        ]
    },
    Messaging => {
        id: "messaging",
        summary: "Publish asynchronous events and user notifications.",
        providers: provider_rows![
            ProviderId::Gcp => (Partial, "monitor::alerts", "Pub/Sub publication for alerts"),
            ProviderId::Slack => (Implemented, "monitor::alerts", "Slack webhook delivery"),
            ProviderId::Telegram => (Implemented, "monitor::alerts", "Telegram Bot API delivery"),
            ProviderId::Sendgrid => (Implemented, "monitor::alerts", "SendGrid email delivery"),
            ProviderId::Skrzynka => (Implemented, "monitor::alerts", "Email from a mailbox Skrzynka holds, through skrzynka message send"),
            ProviderId::Azure => (Planned, "", "No Azure messaging adapter"),
            ProviderId::Aws => (Planned, "", "No AWS messaging adapter"),
        ]
    },
    DataAnalytics => {
        id: "data-analytics",
        summary: "Query operational datasets for usage and product insights.",
        providers: provider_rows![
            ProviderId::Gcp => (Partial, "monitor::billing", "BigQuery queries are implemented for billing exports"),
            ProviderId::Azure => (Planned, "", "No general Azure analytics adapter"),
            ProviderId::Aws => (Planned, "", "No general AWS analytics adapter"),
            ProviderId::Local => (External, "local tools", "No shared Stado analytics service"),
        ]
    },
    Networking => {
        id: "networking",
        summary: "Provide network placement, addressing, and access boundaries.",
        providers: provider_rows![
            ProviderId::Gcp => (Partial, "providers::gcp::inventory + cli::resources", "Networks, firewall rules, and addresses are inspected; selected addresses are managed"),
            ProviderId::Azure => (External, "providers::azure", "Pre-provisioned VNet, subnet, and network security group"),
            ProviderId::Aws => (External, "providers::aws", "Pre-provisioned security group and account networking"),
            ProviderId::Local => (External, "host operating system", "Host networking is consumed but not provisioned"),
        ]
    },
    LoadBalancing => {
        id: "load-balancing",
        summary: "Expose services through stable health-checked endpoints.",
        providers: provider_rows![
            ProviderId::Gcp => (External, "providers::gcp::inventory", "Historical backend services, health checks, and forwarding rules"),
            ProviderId::Azure => (Planned, "", "No Azure load-balancing adapter"),
            ProviderId::Aws => (Planned, "", "No AWS load-balancing adapter"),
            ProviderId::Local => (External, "deployment environment", "Reverse proxy and local routing are managed outside Stado"),
        ]
    },
    MobileAppCapture => {
        id: "mobile-app-capture",
        summary: "Drive and capture installed iOS and Android applications on a declared host.",
        providers: provider_rows![
            ProviderId::Macos => (Implemented, "deploy::mobile_runtime", "Appium with the XCUITest driver for iOS and UiAutomator2 for Android, declared per host under targets[].mobile_runtime and verified at its declared absolute paths"),
            ProviderId::Local => (Partial, "deploy::mobile_runtime", "A registry host carries the runtime only where it declares one; iOS additionally requires Xcode, which no fleet host installs on demand"),
            ProviderId::Linux => (Planned, "", "Android would work through platform-tools; iOS cannot be driven from Linux at all"),
        ]
    },
    IdentityAccess => {
        id: "identity-access",
        summary: "Authenticate workloads and authorize provider operations.",
        providers: provider_rows![
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
        providers: provider_rows![
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
        providers: provider_rows![
            ProviderId::Gcp => (External, "cloudbuild.yaml + providers::gcp::inventory", "Cloud Build configuration exists; execution is external"),
            ProviderId::Azure => (Planned, "", "No Azure build adapter"),
            ProviderId::Aws => (Planned, "", "No AWS build adapter"),
            ProviderId::Local => (Implemented, "deploy::host_build_cache", "Host-local builds and build-cache management"),
        ]
    },
    Observability => {
        id: "observability",
        summary: "Inspect health, logs, heartbeats, failures, and operational state.",
        providers: provider_rows![
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
        providers: provider_rows![
            ProviderId::Gcp => (Implemented, "providers::gcp::inventory", "Compute, storage, IAM, network, and managed-service assets"),
            ProviderId::Azure => (Partial, "cli::resources::inventory", "Stado-owned Azure agent VMs"),
            ProviderId::Aws => (Partial, "cli::resources::inventory", "Stado-owned EC2 agent VMs"),
            ProviderId::Local => (Implemented, "targets + monitor::host_health", "Registered hosts, services, and capacity beacons"),
            ProviderId::Boat => (Partial, "providers::box", "Boat sandbox lifecycle and account limits"),
            ProviderId::Vast => (Partial, "providers::vast", "Published host and marketplace state"),
        ]
    },
    QuotaCapacity => {
        id: "quota-capacity",
        summary: "Report allocatable capacity, quotas, and reservations.",
        providers: provider_rows![
            ProviderId::Stado => (Implemented, "config/quotas.json + queue::capacity", "Provider-neutral reservations and published capacity"),
            ProviderId::Gcp => (Partial, "scheduler::quota", "Preview live accelerator quota reads plus configured reservations"),
            ProviderId::Azure => (Partial, "scheduler::quota", "Configured reservations; live VM-family coverage is incomplete"),
            ProviderId::Aws => (Planned, "", "No live AWS quota adapter"),
            ProviderId::Local => (Implemented, "queue::capacity + providers::local", "Live CPU, RAM, disk, and accelerator measurements"),
            ProviderId::Boat => (Implemented, "providers::box", "Account limits and active sandboxes"),
            ProviderId::Vast => (Partial, "providers::vast", "Published host capacity"),
        ]
    },
    BillingCost => {
        id: "billing-cost",
        summary: "Estimate prices and monitor spend, credits, and billing health.",
        providers: provider_rows![
            ProviderId::Gcp => (Implemented, "scheduler::cost + monitor::billing", "Machine prices, BigQuery export, credits, budgets, and burn"),
            ProviderId::Azure => (Implemented, "scheduler::cost + monitor::billing", "Machine prices, balance, usage, and billing health"),
            ProviderId::Aws => (Partial, "scheduler::cost", "Machine-price estimation without live billing-health collection"),
            ProviderId::Boat => (Partial, "providers::box", "Sandbox cost is provider-owned"),
            ProviderId::Local => (External, "operator", "No cloud bill; hardware cost is outside Stado"),
        ]
    },
    ArtifactDistribution => {
        id: "artifact-distribution",
        summary: "Publish, verify, and retrieve immutable artifacts and releases.",
        providers: provider_rows![
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
        providers: storage::BACKUP_RECOVERY
    },
}
