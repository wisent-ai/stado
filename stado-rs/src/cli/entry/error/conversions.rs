//! How the failures of Stado's own layers - the queue, the store, profiles,
//! configuration, the vault, the registry, JSON and I/O, deploys, leases and
//! service resolution - become a [`CmdError`] stating their class.

use super::{io_failure_code, operating_system_refusal, CmdError};

impl From<String> for CmdError {
    fn from(msg: String) -> Self {
        Self::click(msg)
    }
}

impl From<&str> for CmdError {
    fn from(msg: &str) -> Self {
        Self::click(msg)
    }
}

impl From<crate::queue::submit::SubmitError> for CmdError {
    fn from(exc: crate::queue::submit::SubmitError) -> Self {
        match exc {
            crate::queue::submit::SubmitError::Validation(message) => {
                Self::click(message).stating(crate::primitives::failure::FailureCode::Refused)
            }
            crate::queue::submit::SubmitError::Charge(error) => error,
            crate::queue::submit::SubmitError::Storage(error) => Self::from(error),
            crate::queue::submit::SubmitError::Io(error) => Self::from(error),
        }
    }
}

impl From<crate::queue::StorageError> for CmdError {
    fn from(exc: crate::queue::StorageError) -> Self {
        use crate::primitives::failure::FailureCode;
        use crate::queue::StorageError;
        let code = match &exc {
            StorageError::Http(_) | StorageError::Io(_) | StorageError::Json(_) => None,
            StorageError::NotFound(_) => Some(FailureCode::NotFound),
            StorageError::Auth(_) => Some(FailureCode::Auth),
            StorageError::Held(_) => Some(FailureCode::InfraDown),
            StorageError::StorageConflict(_) | StorageError::PathEscape(_) => {
                Some(FailureCode::Refused)
            }
            StorageError::Gcs { status, .. } | StorageError::Stado { status, .. } => {
                Some(FailureCode::from_upstream_status(*status))
            }
            StorageError::Other(_) => Some(FailureCode::Unknown),
        };
        match (exc, code) {
            (StorageError::Http(error), _) => Self::from(error),
            (StorageError::Io(error), _) => Self::from(error),
            (StorageError::Json(error), _) => Self::from(error),
            (error, Some(code)) => Self::click(error.to_string()).stating(code),
            (error, None) => Self::click(error.to_string()),
        }
    }
}

impl From<crate::profiles::ProfileError> for CmdError {
    fn from(exc: crate::profiles::ProfileError) -> Self {
        match exc {
            crate::profiles::ProfileError::NotFound(message) => {
                Self::click(message).stating(crate::primitives::failure::FailureCode::NotFound)
            }
            crate::profiles::ProfileError::Invalid(message) => {
                Self::click(message).stating(crate::primitives::failure::FailureCode::Config)
            }
            crate::profiles::ProfileError::Io(error) => Self::from(error),
            crate::profiles::ProfileError::Json(error) => Self::from(error),
        }
    }
}

impl From<crate::config_file::ConfigError> for CmdError {
    fn from(exc: crate::config_file::ConfigError) -> Self {
        // A configuration that cannot be read or does not validate is the
        // operator's configuration, whatever the sentence says.
        Self::click(exc.to_string()).stating(crate::primitives::failure::FailureCode::Config)
    }
}

impl From<crate::skarbiec::SkarbiecError> for CmdError {
    fn from(exc: crate::skarbiec::SkarbiecError) -> Self {
        // The vault failure states its own class: a refused grant, an absent
        // item, an unreachable vault and a misconfigured client differ.
        let code = exc.failure_code();
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::targets::RegistryError> for CmdError {
    fn from(exc: crate::targets::RegistryError) -> Self {
        // A registry that does not parse, holds an invalid entry or names one
        // host twice is the fleet's configuration; a registry file that
        // cannot be read states the kernel's kind.
        let code = match &exc {
            crate::targets::RegistryError::Io(_, error) => io_failure_code(error.kind()),
            crate::targets::RegistryError::Json(_)
            | crate::targets::RegistryError::InvalidEntry(_)
            | crate::targets::RegistryError::AmbiguousIdentity { .. } => {
                crate::primitives::failure::FailureCode::Config
            }
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::targets::RegistryFetchError> for CmdError {
    fn from(exc: crate::targets::RegistryFetchError) -> Self {
        // An unreachable store is the store's outage, a store holding no
        // document has none to find, and a document that does not parse is
        // the fleet's configuration.
        let code = match &exc {
            crate::targets::RegistryFetchError::Unreachable { .. } => {
                crate::primitives::failure::FailureCode::InfraDown
            }
            crate::targets::RegistryFetchError::Absent { .. } => {
                crate::primitives::failure::FailureCode::NotFound
            }
            crate::targets::RegistryFetchError::Invalid { .. } => {
                crate::primitives::failure::FailureCode::Config
            }
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::targets::RegistryValidationError> for CmdError {
    fn from(exc: crate::targets::RegistryValidationError) -> Self {
        // A registry document that fails validation is refused: nothing
        // publishes or acts on it.
        Self::refused(exc.to_string())
    }
}

impl From<serde_json::Error> for CmdError {
    fn from(exc: serde_json::Error) -> Self {
        // A document that is not the JSON it must be is refused input; a
        // failure to write or read the stream underneath is the stream's.
        let code = match exc.classify() {
            serde_json::error::Category::Io => crate::primitives::failure::FailureCode::Unknown,
            serde_json::error::Category::Syntax
            | serde_json::error::Category::Data
            | serde_json::error::Category::Eof => crate::primitives::failure::FailureCode::Refused,
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<std::io::Error> for CmdError {
    fn from(exc: std::io::Error) -> Self {
        let message = match operating_system_refusal(&exc) {
            Some(cause) => format!("{exc}; {cause}"),
            None => exc.to_string(),
        };
        Self::click(message).stating(io_failure_code(exc.kind()))
    }
}

impl From<crate::deploy::DeployError> for CmdError {
    /// The class the deploy layer stated where it raised the failure; a
    /// failure raised without one stays unclassified rather than guessed.
    fn from(exc: crate::deploy::DeployError) -> Self {
        let mut converted = Self::click(exc.message);
        converted.failure = exc.failure;
        converted
    }
}

impl From<crate::queue::leases::LeaseError> for CmdError {
    /// A lost race or invalid fence is refused (another holder won); an
    /// illegal transition or unsafe value is refused input; a stored lease
    /// that does not decode is damaged state; a store failure keeps the
    /// store's own class.
    fn from(exc: crate::queue::leases::LeaseError) -> Self {
        use crate::queue::leases::LeaseError;
        match exc {
            LeaseError::Conflict(message) | LeaseError::Value(message) => Self::refused(message),
            LeaseError::Corrupt(message) => Self::unreachable(message),
            LeaseError::Storage(error) => Self::from(error),
        }
    }
}

impl From<crate::registry_import::RegistryImportError> for CmdError {
    /// A canonical registry that is already invalid is the fleet's
    /// declaration; a store that cannot be opened, written or read back the
    /// same is the store's outage.
    fn from(exc: crate::registry_import::RegistryImportError) -> Self {
        use crate::primitives::failure::FailureCode;
        use crate::registry_import::RegistryImportError;
        let code = match &exc {
            RegistryImportError::CanonicalInvalid { .. } => FailureCode::Config,
            RegistryImportError::Storage(_) | RegistryImportError::Verification => {
                FailureCode::InfraDown
            }
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::service_resolution::ResolveError> for CmdError {
    /// A malformed directory is Config, an undeclared service NotFound, a
    /// consumer the service does not admit Refused, and a service held by a
    /// placement move InfraDown: the one answer a later call can change.
    fn from(exc: crate::service_resolution::ResolveError) -> Self {
        use crate::primitives::failure::FailureCode;
        use crate::service_resolution::ResolveError;
        let code = match &exc {
            ResolveError::Declaration(_) => FailureCode::Config,
            ResolveError::UnknownService(_) => FailureCode::NotFound,
            ResolveError::Unauthorized { .. } => FailureCode::Refused,
            ResolveError::Moving { .. } => FailureCode::InfraDown,
        };
        Self::click(exc.to_string()).stating(code)
    }
}
