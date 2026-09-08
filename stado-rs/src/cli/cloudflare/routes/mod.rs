//! The four route operations behind the command surface: the zone inventory,
//! one hostname's status, the upsert and the removal — plus the tunnel ingress
//! rules all four read or edit.

mod ingress;
mod inspect;
mod list;
mod mutate;

pub(super) use inspect::route_status;
pub(super) use list::list_routes;
pub(super) use mutate::{remove_route, route_tunnel};
