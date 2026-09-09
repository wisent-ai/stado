//! The `stado fleet` parser surface and its dispatch, one component per
//! command tree plus the runner that translates the fleet's verdict.

mod dispatch;
mod fleet_commands;
mod ingress_commands;
mod key_commands;

pub use dispatch::run;
pub use fleet_commands::FleetCommands;
pub use ingress_commands::IngressCommands;
pub use key_commands::KeyCommands;
