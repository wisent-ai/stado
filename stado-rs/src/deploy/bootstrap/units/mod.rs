//! Bootstrap stage two: render the remote agent systemd unit and the command
//! that writes, unmasks, enables and restarts it.

mod command;
mod text;

pub use command::AGENT_UNIT;
pub(super) use command::{agent_install, remote_home};
