//! `stado route` — routing operations derived from the service directory.
//!
//! A service name resolves once, through `service_directory.services`; the
//! command never carries a product-to-host or product-to-port table of its own.

mod command;
mod directory;
mod forward;
mod inspect;

pub use command::{dispatch, RouteCommands, RoutePlacementCommands};
