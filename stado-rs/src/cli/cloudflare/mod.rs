//! Native Cloudflare Tunnel route management through credentials held by Stado.
//!
//! Inventory and status compare tunnel ingress, exact DNS records and active
//! connector sessions without claiming that the connector can reach its origin.
//! Upsert configures ingress before moving DNS. Removal deletes only matching
//! tunnel DNS before ingress and preserves the shared connector. Secret values
//! stay in memory and are never rendered in command output.
//!
//! `command` holds the declared surface and its dispatch table, `api` the
//! authenticated calls every operation is made of, `records` the tunnel and
//! DNS state a route is compared against, and `routes` the four operations
//! themselves.

mod api;
mod command;
mod records;
mod routes;

pub use command::{dispatch, CloudflareCommands, TunnelScopeArgs};
