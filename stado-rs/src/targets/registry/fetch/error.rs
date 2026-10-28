use crate::targets::*;

/// Why the canonical registry could not be READ — as distinct from a
/// registry that WAS read and simply does not list a given entry.
///
/// The split is load-bearing. The coordinator's rogue-daemon kill switch
/// (`coordinator::run`) exits the process when a registry it successfully
/// read omits its entry, and must keep running when it could not read one
/// at all. Collapsing both into an empty registry is what took the fleet
/// down when the GCP billing account was closed: every GCS call started
/// answering `accountDisabled`, and the kill switch fired fleet-wide
/// against a registry nobody had touched.
#[derive(Debug, thiserror::Error)]
pub enum RegistryFetchError {
    /// The store refused or failed the read (auth, network, disabled
    /// billing account, ...). Says NOTHING about the registry's contents.
    #[error("registry store unreachable ({location}): {detail}")]
    Unreachable {
        /// Where the read was attempted, per `registry_location`.
        location: String,
        /// The underlying store error.
        detail: String,
    },
    /// The store answered, but holds no registry document. Just as
    /// non-authoritative about any single entry: a container nobody has
    /// seeded yet looks exactly like this, and the documented kill switch
    /// is "operator removed the ENTRY", never "operator deleted the whole
    /// registry".
    #[error("no registry document at {location}")]
    Absent {
        /// Where the read was attempted, per `registry_location`.
        location: String,
    },
    /// A document came back that is not a valid registry, so its contents
    /// cannot be trusted to revoke anything.
    #[error("invalid registry document at {location}: {source}")]
    Invalid {
        /// Where the document was read from, per `registry_location`.
        location: String,
        /// The parse failure.
        source: RegistryError,
    },
}
