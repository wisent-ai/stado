//! Serving, publishing and signing releases read the vault through Stado's
//! Skarbiec identity.

mod publishers;

pub use publishers::*;

/// Active authenticated software publishers. Public readers use the separate
/// tokenless release GET route.
pub const ACTIVE_RELEASE_PUBLISHERS: &[&str] = &[
    "brama",
    "compute-marketplace",
    "image-video-router",
    "oko",
    "skarbiec",
    "stado",
    "trading-autonomy",
    "wisent-backend",
];
