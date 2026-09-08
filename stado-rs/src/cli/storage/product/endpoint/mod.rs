//! The HTTPS client every object request shares, the origins this process
//! may address, and how one object route is built.

pub(in crate::cli::storage) mod client;
pub(in crate::cli::storage) mod origin;
pub(in crate::cli::storage) mod route;
