//! How the failures of Stado's services - the dashboard, the bundled control
//! plane, the failure fixer, the release control socket and client, release
//! artifacts, host health and inference plans - become a [`CmdError`].

use super::{io_failure_code, CmdError};

impl From<crate::dashboard::DashboardError> for CmdError {
    /// The store and the socket keep the classes their own conversions
    /// state, a listener the product will not serve on is refused, and a
    /// failure raised as a sentence states none.
    fn from(exc: crate::dashboard::DashboardError) -> Self {
        match exc {
            crate::dashboard::DashboardError::Storage(error) => Self::from(error),
            crate::dashboard::DashboardError::Io(error) => Self::from(error),
            crate::dashboard::DashboardError::Refused(message) => {
                Self::click(message).stating(crate::primitives::failure::FailureCode::Refused)
            }
            crate::dashboard::DashboardError::Other(message) => Self::click(message),
        }
    }
}

impl From<crate::remote::control_plane::ControlPlaneError> for CmdError {
    /// A bundled coordinator's declaration it cannot run with is config;
    /// the vault keeps the class its own conversion states.
    fn from(exc: crate::remote::control_plane::ControlPlaneError) -> Self {
        use crate::remote::control_plane::ControlPlaneError;
        match exc {
            ControlPlaneError::Config(message) => {
                Self::click(message).stating(crate::primitives::failure::FailureCode::Config)
            }
            ControlPlaneError::Vault(error) => Self::from(error),
        }
    }
}

impl From<crate::failure_fixer::FixError> for CmdError {
    /// The failure fixer's store, state file, filesystem and vault failures
    /// keep the classes their own conversions state.
    fn from(exc: crate::failure_fixer::FixError) -> Self {
        use crate::failure_fixer::FixError;
        match exc {
            FixError::Storage(error) => Self::from(error),
            FixError::Json(error) => Self::from(error),
            FixError::Io(error) => Self::from(error),
            FixError::Skarbiec(error) => Self::from(error),
        }
    }
}

impl From<crate::release_agent::rollout::serving::control::ControlSocketError> for CmdError {
    /// An operating-system failure states its kind with the step it met; a
    /// control path this process will not take over is refused; a location
    /// it cannot derive is config; a proxied route that failed while it
    /// served is the outage of the service behind it.
    fn from(exc: crate::release_agent::rollout::serving::control::ControlSocketError) -> Self {
        use crate::primitives::failure::FailureCode;
        use crate::release_agent::rollout::serving::control::ControlSocketError;
        let message = exc.to_string();
        let code = match &exc {
            ControlSocketError::Io { error, .. } => io_failure_code(error.kind()),
            ControlSocketError::Refused(_) => FailureCode::Refused,
            ControlSocketError::Config(_) => FailureCode::Config,
            ControlSocketError::RouteFailed(_) => FailureCode::InfraDown,
        };
        Self::click(message).stating(code)
    }
}

impl From<crate::release_agent::rollout::serving::control::ControlClientError> for CmdError {
    /// A socket location or bind that is not usable is config; an
    /// operating-system failure states its kind; a host process that is not
    /// running is its outage; a socket, peer or answer this client will not
    /// accept, or an owner refusing the operation, is refused.
    fn from(exc: crate::release_agent::rollout::serving::control::ControlClientError) -> Self {
        use crate::primitives::failure::FailureCode;
        use crate::release_agent::rollout::serving::control::ControlClientError;
        let code = match &exc {
            ControlClientError::Config(_) => FailureCode::Config,
            ControlClientError::Io { error, .. } => io_failure_code(error.kind()),
            ControlClientError::Unavailable(_) => FailureCode::InfraDown,
            ControlClientError::Refused(_) => FailureCode::Refused,
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::release_control::ArtifactDigestError> for CmdError {
    /// A release artifact that cannot be opened or read fails by its io kind;
    /// one that is not a non-empty regular file is refused.
    fn from(exc: crate::release_control::ArtifactDigestError) -> Self {
        use crate::primitives::failure::FailureCode;
        use crate::release_control::ArtifactDigestError;
        let code = match &exc {
            ArtifactDigestError::Io { error, .. } => io_failure_code(error.kind()),
            ArtifactDigestError::NotARegularFile(_) => FailureCode::Refused,
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::monitor::host_health::HostHealthError> for CmdError {
    /// A host the registry does not hold and a host with no beacon are not
    /// found; a host that is not local is refused; a beacon that is not the
    /// JSON object it must be is damaged data; the registry and the store
    /// keep the classes their own conversions state.
    fn from(exc: crate::monitor::host_health::HostHealthError) -> Self {
        use crate::monitor::host_health::HostHealthError;
        use crate::primitives::failure::FailureCode;
        let message = exc.to_string();
        let code = match exc {
            HostHealthError::RegistryFetch(error) => return Self::from(error),
            HostHealthError::Registry(error) => return Self::from(error),
            HostHealthError::Storage(error) => return Self::from(error),
            HostHealthError::UnknownTarget(_) | HostHealthError::NoBeacon { .. } => {
                FailureCode::NotFound
            }
            HostHealthError::NotLocal(_) => FailureCode::Refused,
            HostHealthError::InvalidJson { .. } | HostHealthError::NotAnObject { .. } => {
                FailureCode::InfraDown
            }
        };
        Self::click(message).stating(code)
    }
}

impl From<crate::inference::plan::PlanError> for CmdError {
    /// An unset `HOME` is the environment's, an id the command never printed
    /// is the operator's input, a file operation fails by its kind, and a
    /// plan file that does not hold its plan is damaged data.
    fn from(exc: crate::inference::plan::PlanError) -> Self {
        use crate::inference::plan::PlanError;
        use crate::primitives::failure::FailureCode;
        let code = match &exc {
            PlanError::NoHome => FailureCode::Config,
            PlanError::InvalidId(_) => return Self::usage(exc.to_string()),
            PlanError::Encode(_) => FailureCode::Unknown,
            PlanError::Read(_, error) | PlanError::Write(_, error) => io_failure_code(error.kind()),
            PlanError::Invalid(..) | PlanError::Mismatched(_) => FailureCode::InfraDown,
        };
        Self::click(exc.to_string()).stating(code)
    }
}
