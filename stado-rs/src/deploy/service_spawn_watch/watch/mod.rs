//! The watch itself: what is sent to a host, and what comes back.

mod parse;
mod spawns;

pub use self::parse::parse_watch;
pub use self::spawns::watch_spawns;
