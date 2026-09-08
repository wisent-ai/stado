//! The watch itself: what is sent to a host, and what comes back.

mod parse;
mod spawns;

#[cfg(test)]
mod gap_argument_rendering;
#[cfg(test)]
mod marker_stream_parsing;

pub use self::parse::parse_watch;
pub use self::spawns::watch_spawns;
