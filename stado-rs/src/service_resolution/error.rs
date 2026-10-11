//! Why a consumer gets no route to a service.

/// Why a consumer gets no route to a service. Each variant is a different
/// answer for the caller: a directory that is not the declaration it must be,
/// a service nobody declared, a consumer the service does not admit, and a
/// service held by a placement move that a later call can reach.
#[derive(Debug)]
pub enum ResolveError {
    /// The directory, or a record resolution reads, is malformed or absent.
    Declaration(String),
    /// The directory declares no service by this name.
    UnknownService(String),
    /// The service does not admit this consumer.
    Unauthorized { service: String, consumer: String },
    /// A placement transaction holds the service's profile.
    Moving { service: String, profile: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declaration(detail) => f.write_str(detail),
            Self::UnknownService(service) => write!(f, "unknown logical service {service:?}"),
            Self::Unauthorized { service, consumer } => write!(
                f,
                "consumer {consumer:?} is not authorized for service {service:?}"
            ),
            Self::Moving { service, profile } => write!(
                f,
                "service {service:?} is unavailable during placement transaction for {profile:?}"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

impl From<ResolveError> for String {
    fn from(error: ResolveError) -> Self {
        error.to_string()
    }
}
