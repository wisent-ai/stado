//! The gates one tick passes between advancing its slots and scanning the
//! queue: the release-drift and inference reservation checks
//! ([`inference`]), the measured disk, settling and driver state
//! ([`resources`]), and the broadcast that decides whether this host admits
//! anything at all ([`admission`]).

pub mod admission;
pub mod inference;
pub mod resources;
