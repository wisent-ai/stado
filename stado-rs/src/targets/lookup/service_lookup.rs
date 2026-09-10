use crate::targets::*;

/// The stable code a machine caller branches on when the service directory it
/// is holding is older than the one the authority published.
///
/// It exists because the alternative was silence: a consumer with a stale
/// directory dials the previous active host, gets a refused connection, and
/// reports "connection refused" — which sends the operator to the network
/// instead of to `stado registry pull`.
pub const SERVICE_DIRECTORY_STALE_CODE: &str = "SERVICE_DIRECTORY_STALE";

/// Why a service could not be resolved from the directory. Every variant is
/// a refusal: a lookup NEVER falls back to a guessed host or a default port,
/// because both produce a call to the wrong process rather than an error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceDirectoryError {
    #[error(
        "{code}: the cached service directory is generation {cached}, the authority \
         published generation {authority}; refresh it (stado registry pull) and retry — \
         the cached endpoint for '{service}' may name a host that has already handed \
         the service over",
        code = SERVICE_DIRECTORY_STALE_CODE
    )]
    Stale {
        service: String,
        cached: u64,
        authority: u64,
    },
    #[error("service '{0}' is not declared in the service directory")]
    UnknownService(String),
    #[error(
        "service '{service}' declares active host '{host}', which has no endpoint in the \
         service directory"
    )]
    NoEndpoint { service: String, host: String },
}

impl ServiceDirectoryError {
    /// The stable code a machine caller branches on, in the spelling
    /// `machine::MachineError` emits.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Stale { .. } => SERVICE_DIRECTORY_STALE_CODE,
            Self::UnknownService(_) => "SERVICE_NOT_IN_DIRECTORY",
            Self::NoEndpoint { .. } => "SERVICE_ENDPOINT_MISSING",
        }
    }

    /// A stale cache is fixed by re-reading the directory, so the same call
    /// is worth making again. A service the directory does not declare is
    /// not: that one needs an edit.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Stale { .. })
    }
}

impl ServiceDirectory {
    /// The URL a consumer should call for `service`, fenced against the
    /// generation the authority last published.
    ///
    /// The generation is checked FIRST: a stale directory that happens to
    /// still name a reachable endpoint is the dangerous case, because the
    /// call succeeds against the host that no longer owns the service.
    pub fn endpoint(
        &self,
        service: &str,
        authority_generation: u64,
    ) -> Result<&str, ServiceDirectoryError> {
        self.require_generation(service, authority_generation)?;
        let entry = self
            .services
            .get(service)
            .ok_or_else(|| ServiceDirectoryError::UnknownService(service.to_string()))?;
        entry
            .address_for(&entry.active_host)
            .map(|endpoint| endpoint.url.as_str())
            .ok_or_else(|| ServiceDirectoryError::NoEndpoint {
                service: service.to_string(),
                host: entry.active_host.clone(),
            })
    }

    /// Refuse a directory older than the generation the authority published.
    pub fn require_generation(
        &self,
        service: &str,
        authority_generation: u64,
    ) -> Result<(), ServiceDirectoryError> {
        if self.generation < authority_generation {
            return Err(ServiceDirectoryError::Stale {
                service: service.to_string(),
                cached: self.generation,
                authority: authority_generation,
            });
        }
        Ok(())
    }
}
