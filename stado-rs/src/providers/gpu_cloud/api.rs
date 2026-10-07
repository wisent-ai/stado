//! The contract every GPU cloud vendor adapter implements: rent one machine,
//! release it, read one, list them. Everything Stado decides — names, the
//! guest hostname, which machines are agents, how many run per accelerator,
//! when a release counts as done — lives once in `provider`, above this line.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::capabilities::GpuCloudVendor;

/// What the dispatcher asks one vendor to rent.
#[derive(Debug, Clone, Copy)]
pub struct LaunchRequest<'a> {
    /// The agent's name (`wisent-agent-<accel>-<tick>-<n>`), lowercase
    /// letters, digits and dashes only. Vendors that name machines use it.
    pub name: &'a str,
    /// The vendor's own instance type, plan, flavor or shape, taken from the
    /// vendor's sizing ladder.
    pub instance_type: &'a str,
    /// The scheduler's accelerator name (`nvidia-h100-80gb`) the type carries.
    pub accel_type: &'a str,
    /// Boot or container disk in GB, from the job.
    pub boot_disk_gb: i64,
    /// The complete bash script the machine must run once as root at first
    /// boot, already carrying the hostname line when the guest is a VM.
    pub startup_script: &'a str,
}

/// One machine as the vendor reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    /// The vendor's identifier for the machine, exactly as its API takes it
    /// back. Must not contain `@`; a vendor that needs a location to address a
    /// machine encodes it here (`fr-par-2/<uuid>`).
    pub native_id: String,
    /// The name the machine was launched with.
    pub name: String,
    /// The vendor's instance type, plan, flavor or shape.
    pub instance_type: String,
    pub state: MachineState,
    /// When the vendor created it; `None` when the vendor does not say.
    pub created_at: Option<DateTime<Utc>>,
}

/// A vendor's lifecycle state, mapped once by each adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineState {
    /// Accepted and booting; billed or about to be.
    Provisioning,
    Running,
    Stopping,
    /// Stopped with its disk kept; not running work.
    Stopped,
    /// Being released.
    Terminating,
    /// Released; the vendor may still list it for a while.
    Terminated,
    /// The vendor gave up on it.
    Failed,
}

impl MachineState {
    /// Alive in the scheduler's sense: it holds capacity and can run an agent.
    pub const fn alive(self) -> bool {
        matches!(self, Self::Provisioning | Self::Running)
    }

    /// The lifecycle vocabulary the reaper reads (`Provider::instance_lifecycle_state`).
    pub const fn lifecycle(self) -> &'static str {
        match self {
            Self::Provisioning => "PROVISIONING",
            Self::Running => "RUNNING",
            Self::Stopping => "STOPPING",
            Self::Stopped => "STOPPED",
            Self::Terminating => "STOPPING",
            Self::Terminated | Self::Failed => "TERMINATED",
        }
    }
}

/// How a vendor failed. Each variant carries the vendor's own words.
#[derive(Debug, thiserror::Error)]
pub enum GpuCloudError {
    /// The vendor has no capacity for this type in the configured location.
    /// The dispatcher then tries the next tier; it is not a fault.
    #[error("{vendor}: no capacity for {instance_type}: {detail}")]
    Capacity {
        vendor: &'static str,
        instance_type: String,
        detail: String,
    },
    /// The machine does not exist (any more).
    #[error("{vendor}: {operation}: not found: {detail}")]
    NotFound {
        vendor: &'static str,
        operation: String,
        detail: String,
    },
    /// The vendor refused the credential.
    #[error("{vendor}: {operation}: the credential in Skarbiec role {role} was refused (HTTP {status}): {detail}")]
    Unauthorized {
        vendor: &'static str,
        operation: String,
        role: String,
        status: u16,
        detail: String,
    },
    /// Any other answer outside 2xx.
    #[error("{vendor}: {operation}: HTTP {status}: {detail}")]
    Api {
        vendor: &'static str,
        operation: String,
        status: u16,
        detail: String,
    },
    /// The request never got an answer.
    #[error("{vendor}: {operation}: {detail}")]
    Transport {
        vendor: &'static str,
        operation: String,
        detail: String,
    },
    /// The vendor answered with something this adapter cannot read.
    #[error("{vendor}: {operation}: unexpected answer: {detail}")]
    Response {
        vendor: &'static str,
        operation: String,
        detail: String,
    },
    /// The credential could not be read from Skarbiec.
    #[error("{0}")]
    Credential(String),
    /// A required setting is missing or malformed.
    #[error("{0}")]
    Configuration(String),
}

impl GpuCloudError {
    pub fn response(vendor: GpuCloudVendor, operation: &str, detail: impl Into<String>) -> Self {
        Self::Response {
            vendor: vendor.display_name(),
            operation: operation.to_string(),
            detail: detail.into(),
        }
    }

    pub fn capacity(vendor: GpuCloudVendor, instance_type: &str, detail: impl Into<String>) -> Self {
        Self::Capacity {
            vendor: vendor.display_name(),
            instance_type: instance_type.to_string(),
            detail: detail.into(),
        }
    }

    /// The HTTP status of an API refusal, when there was one.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } | Self::Unauthorized { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The vendor's own text of an API refusal, when there was one.
    pub fn detail(&self) -> &str {
        match self {
            Self::Capacity { detail, .. }
            | Self::NotFound { detail, .. }
            | Self::Unauthorized { detail, .. }
            | Self::Api { detail, .. }
            | Self::Transport { detail, .. }
            | Self::Response { detail, .. } => detail,
            Self::Credential(detail) | Self::Configuration(detail) => detail,
        }
    }
}

/// One vendor's API. Implementations are thin: they translate Stado's request
/// into the vendor's call and the vendor's answer into [`Machine`], and they
/// map the vendor's capacity and not-found answers onto
/// [`GpuCloudError::Capacity`] and [`GpuCloudError::NotFound`].
#[async_trait]
pub trait GpuCloudApi: Send + Sync {
    /// Rent one machine and return it as the vendor reports it right after
    /// acceptance. A capacity refusal is [`GpuCloudError::Capacity`].
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError>;

    /// Release a machine for good, disks included, so it stops billing.
    /// A machine that is already gone is [`GpuCloudError::NotFound`].
    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError>;

    /// Read one machine; `None` when the vendor no longer knows it.
    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError>;

    /// Every machine the credential can see, in any state, every page read.
    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError>;
}
