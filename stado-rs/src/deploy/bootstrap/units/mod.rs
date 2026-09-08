//! Bootstrap stage two: render the remote systemd units and the commands
//! that write, unmask, enable and restart them.

mod command;
mod text;

pub(super) use command::unit_installs;
