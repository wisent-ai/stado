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
    /// A placement transaction holds the service's profile. The transaction
    /// is carried whole, so the refusal names which move holds the service,
    /// between which hosts and since when, and `stado placement release`
    /// can end it when its process ended.
    Moving {
        service: String,
        transaction: crate::placement::PlacementTransaction,
    },
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
            Self::Moving {
                service,
                transaction,
            } => write!(
                f,
                "service {service:?} is unavailable during placement transaction {} for {:?}, moving it from {} to {} since {}; stado placement transactions lists it, and stado placement release {} ends it if its move is no longer running",
                transaction.id,
                transaction.profile,
                transaction.from_host,
                transaction.to_host,
                transaction.started_at,
                transaction.id
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
