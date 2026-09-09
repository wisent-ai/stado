//! The fixed remote programs of the scratch capability, and their answers.
//!
//! Each program is a compile-time constant fed to `/bin/sh -c` with quoted
//! arguments, the shape `host user create` already uses on this channel: no
//! registry value and no operator string is ever concatenated into shell, and
//! the privileged half is escalated exactly the way account creation escalates
//! it.
//!
//! Every program answers in the tab-delimited `STADO_*` marker protocol the
//! rest of the host channel speaks, so one parser family covers them all.

mod answers;
mod programs;

pub use answers::{parse_leases, parse_marker, parse_state, HostLease, HostState};
pub use programs::{
    forget_command, list_command, record_command, state_command, trust_command, STATUS_PREFIX,
};
