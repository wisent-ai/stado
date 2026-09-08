//! The submitter-facing half of the coverage contract: the entries a
//! universe promises, the verifiers that check them, and the registry the
//! CLI resolves a universe id through.

mod entry;
mod registry;
mod verifiers;

pub use entry::UniverseEntry;
pub use registry::{
    build_universe, list_universes, register_universe, registered_universe_names,
    unknown_universe_message, Universe, UniverseFactory,
};
pub use verifiers::{StadoObjectExistsVerifier, URIExistsVerifier, Verifier};
