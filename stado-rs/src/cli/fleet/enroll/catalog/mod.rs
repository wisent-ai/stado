//! The central enrollment and communication catalog.
//!
//! One optional top-level `enrollment` section of the canonical registry
//! declares which registration paths the fleet allows, and a `channels`
//! section declares how machines reach the control plane. Both are parsed
//! here, enforced in the preflights of `join`/`approve`/`enroll`, and
//! rendered by `stado fleet catalog`. A document without the sections is
//! unrestricted — and says so out loud when the catalog is printed, so an
//! absent policy is never mistaken for a declared one.

mod gates;
mod methods;
mod report;
mod sections;

pub use gates::{
    require_adopt_allowed, require_enroll_allowed, require_invite_allowed, require_join_allowed,
};
pub use methods::methods;
pub use report::catalog;
pub use sections::{parse_channels, parse_enrollment, ChannelsCatalog, EnrollmentCatalog};
